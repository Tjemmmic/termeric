//! Harvest mode rendering — growth indicators, root cellar, sprout menu
//!
//! Three `pub(super)` functions called from `build_render_data()`:
//! - `build_harvest_indicators()` — draw growth phase characters in the left gutter
//! - `build_root_cellar()` — slide-in command history panel
//! - `build_sprout_menu()` — context menu for command actions

use super::Renderer;
use crate::renderer::types::{CellarRenderInfo, IndicatorRenderInfo, SproutMenuRenderInfo};
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::constants::BORDER_WIDTH;
use crate::ui::icon::Icon;

/// Return the sprout action row under a pointer. Geometry intentionally uses
/// the same padding and item-height math as `build_sprout_menu`.
pub fn sprout_menu_hit_test(menu: &SproutMenuRenderInfo, x: f32, y: f32) -> Option<usize> {
    menu.layout.hit_test(x, y)
}

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
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        // Position indicators in the left padding, one cell-width left of the grid
        let gutter_x = self.tab_bar_width() + (self.padding - cell_width).max(0.0);

        for ind in indicators {
            let y = content_y + ind.row as f32 * cell_height;
            let color = scale_alpha(ind.color, ind.opacity);

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
        window_width: f32,
        window_height: f32,
    ) {
        if cellar.animation_progress < 0.001 {
            return;
        }

        let anim = cellar.animation_progress;
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let layout = &cellar.layout;
        let panel_x = layout.panel.x;
        let panel_y = layout.panel.y;
        let panel_width = layout.panel.width;

        let bg = with_alpha(self.ui.menu_bg, 0.95 * anim);
        let border = with_alpha(self.ui.tab_border, 0.5 * anim);
        let label_size = self.ui_label_px();

        // Scrim (dim area behind panel)
        self.rect_pipeline.push_rect(
            0.0,
            0.0,
            window_width,
            window_height,
            scale_alpha(self.ui.scrim, anim * 0.5),
        );
        self.draw_card(layout.panel, bg, border);

        // Header
        let header_y = panel_y + cell_height * 0.5;
        let text_color = with_alpha(self.ui.menu_text, anim);
        let dim_color = with_alpha(self.ui.menu_text_dim, anim);
        let text_width = panel_width - cell_width * 2.0;
        self.push_ui_label(
            "Root Cellar",
            panel_x + cell_width,
            header_y,
            cell_height,
            text_width,
            label_size,
            text_color,
        );
        let [close_x, close_y] = layout.close.center();
        self.push_icon(Icon::Close, close_x, close_y, text_color);

        // Success rate badge
        let rate_text = if cellar.evicted_entries == 0 {
            format!(
                "{:.0}% healthy · {} kept",
                cellar.success_rate * 100.0,
                cellar.total_entries
            )
        } else {
            format!(
                "{:.0}% healthy · {} kept · {} evicted",
                cellar.success_rate * 100.0,
                cellar.total_entries,
                cellar.evicted_entries
            )
        };
        let rate_color = if cellar.success_rate > 0.8 {
            with_alpha(self.ui.success, anim)
        } else if cellar.success_rate > 0.5 {
            with_alpha(self.ui.warning, anim)
        } else {
            with_alpha(self.ui.error, anim)
        };
        self.push_ui_label(
            &rate_text,
            panel_x + cell_width,
            header_y + cell_height,
            cell_height,
            text_width,
            label_size,
            rate_color,
        );

        // Separator
        self.rect_pipeline.push_rect(
            panel_x + cell_width * 0.5,
            layout.list.y - cell_height * 0.5,
            panel_width - cell_width,
            BORDER_WIDTH,
            border,
        );

        // Entry list
        for (entry, (_, row)) in cellar.entries.iter().zip(&layout.rows) {
            let entry_y = row.y;
            let entry_color = with_alpha(entry.health_color, anim);

            if cellar.selected_id == Some(entry.id) {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
                    with_alpha(self.ui.palette_accent, 0.14 * anim),
                    4.0,
                );
            }

            // Health indicator character
            let health_char = match entry.health {
                0 => '\u{25CF}', // filled circle (healthy)
                1 => '\u{25CB}', // empty circle (warning)
                _ => '\u{2716}', // heavy X (sick)
            };
            self.text_pipeline.push_char(
                health_char,
                row.x + cell_width * 0.5,
                entry_y,
                entry_color,
            );

            // Exit code, right-aligned
            let mut code_text = match entry.exit_code {
                Some(0) => "ok".to_string(),
                Some(code) => format!("E{}", code),
                None => "...".to_string(),
            };
            if entry.output_truncated {
                code_text.push_str(" clipped");
            }
            let code_width = self.ui_text_width(&code_text, label_size);
            let code_x = row.x + row.width - code_width - cell_width * 0.5;
            self.push_ui_label(
                &code_text,
                code_x,
                entry_y,
                cell_height,
                code_width,
                label_size,
                dim_color,
            );

            let command_x = row.x + cell_width * 2.0;
            self.push_ui_label(
                &entry.command,
                command_x,
                entry_y,
                cell_height,
                code_x - command_x - cell_width * 0.5,
                label_size,
                text_color,
            );
        }

        self.draw_scrollbar(
            layout.scrollbar.x,
            layout.scrollbar.y,
            layout.scrollbar.height,
            cellar.total_entries,
            layout.max_visible,
            cellar.scroll,
            anim,
            4.0,
        );
    }

    /// Draw the sprout context menu at a click position.
    ///
    /// Shows available actions for a command region (copy, rerun, explain, etc.)
    pub(super) fn build_sprout_menu(&mut self, menu: &SproutMenuRenderInfo) {
        if menu.opacity < 0.001 {
            return;
        }

        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let anim = menu.opacity;

        let bg = with_alpha(self.ui.palette_bg, 0.95 * anim);
        let border = with_alpha(self.ui.palette_border, 0.6 * anim);
        let text_color = with_alpha(self.ui.palette_text, anim);
        let highlight_bg = with_alpha(self.ui.palette_accent, 0.15 * anim);
        let icon_color = with_alpha(self.ui.palette_accent, 0.8 * anim);

        let label_size = self.ui_label_px();

        // Card shadow + background
        let panel = menu.layout.panel;
        self.draw_card_shadow(panel, 0.2, 2);
        self.draw_card(panel, bg, border);

        // Action items
        for (i, ((name, icon, enabled), row)) in
            menu.actions.iter().zip(&menu.layout.rows).enumerate()
        {
            // Highlight selected item
            if i == menu.selected && *enabled {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
                    highlight_bg,
                    4.0,
                );
            }

            self.text_pipeline.push_char(
                *icon,
                row.x + menu.layout.padding,
                row.y + (row.height - cell_height) / 2.0,
                if *enabled {
                    icon_color
                } else {
                    with_alpha(self.ui.palette_text, 0.35 * anim)
                },
            );

            let name_x = row.x + menu.layout.padding + cell_width * 2.0;
            self.push_ui_label(
                name,
                name_x,
                row.y,
                row.height,
                row.right() - name_x,
                label_size,
                if *enabled {
                    text_color
                } else {
                    with_alpha(self.ui.palette_text, 0.45 * anim)
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprout_hit_test_matches_rendered_rows() {
        let menu = SproutMenuRenderInfo {
            actions: vec![("Copy".into(), 'c', true), ("Run".into(), 'r', false)],
            selected: 0,
            opacity: 1.0,
            layout: crate::harvest::SproutMenuLayout::calculate(
                0, 0, 2, 10, 400.0, 300.0, 0.0, 20.0, 10.0, 10.0, 10.0,
            ),
        };
        let first = menu.layout.rows[0];
        let second = menu.layout.rows[1];
        assert_eq!(
            sprout_menu_hit_test(&menu, first.x + 1.0, first.y + 1.0),
            Some(0)
        );
        assert_eq!(
            sprout_menu_hit_test(&menu, second.x + 1.0, second.y + 1.0),
            Some(1)
        );
        assert_eq!(sprout_menu_hit_test(&menu, 1.0, 1.0), None);
    }
}
