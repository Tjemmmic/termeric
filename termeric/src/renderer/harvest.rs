//! Harvest mode rendering — growth indicators, root cellar, sprout menu
//!
//! Three `pub(super)` functions called from `build_render_data()`:
//! - `build_harvest_indicators()` — draw growth phase characters beside their pane's rows
//! - `build_root_cellar()` — slide-in command history panel
//! - `build_sprout_menu()` — context menu for command actions

use super::Renderer;
use super::colors::UiPalette;
use crate::harvest::{CommandHealth, GrowthPhase};
use crate::renderer::types::{CellarRenderInfo, IndicatorRenderInfo, SproutMenuRenderInfo};
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::constants::{BORDER_WIDTH, BUTTON_CORNER_RADIUS};
use crate::ui::icon::Icon;

impl Renderer {
    /// Draw growth indicators beside the rows their commands started on.
    ///
    /// Each indicator is a single character (seedling, sprout, flower, etc.)
    /// in the cell its pane's placement gives it, colored by growth phase.
    pub(super) fn build_harvest_indicators(&mut self, indicators: &[IndicatorRenderInfo]) {
        for indicator in indicators {
            let color = scale_alpha(growth_color(&self.ui, indicator.phase), indicator.opacity);
            self.text_pipeline.push_char(
                indicator.phase.character(),
                indicator.cell.x,
                indicator.cell.y,
                color,
            );
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
            self.px(BORDER_WIDTH),
            border,
        );

        // Entry list
        for (entry, (_, row)) in cellar.entries.iter().zip(&layout.rows) {
            let entry_y = row.y;
            let (health_char, health_color) = health_marker(&self.ui, entry.health);

            if cellar.selected_id == Some(entry.id) {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
                    with_alpha(self.ui.palette_accent, 0.14 * anim),
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }

            self.text_pipeline.push_char(
                health_char,
                row.x + cell_width * 0.5,
                entry_y,
                scale_alpha(health_color, anim),
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
            layout.scrollbar,
            layout.max_visible,
            cellar.total_entries,
            cellar.scroll,
            anim,
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
        self.draw_card_shadow(panel, 0.2 * anim, 2);
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
                    self.px(BUTTON_CORNER_RADIUS),
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

/// The marker a cellar row draws for how its command fared, in the theme's
/// status colors: a filled dot for success, a cross for failure, and a
/// muted ring while it runs.
fn health_marker(ui: &UiPalette, health: CommandHealth) -> (char, [f32; 4]) {
    match health {
        CommandHealth::Healthy => ('\u{25CF}', ui.success),
        CommandHealth::Wilted => ('\u{2716}', ui.error),
        CommandHealth::Running => ('\u{25CB}', ui.menu_text_dim),
    }
}

/// A growth indicator's color in the theme's status colors: a running
/// command grows from the muted foreground into green, and a finished one
/// blooms gold or withers red.
fn growth_color(ui: &UiPalette, phase: GrowthPhase) -> [f32; 4] {
    match phase {
        GrowthPhase::Seed => with_alpha(ui.foreground, 0.6),
        GrowthPhase::Sprouting => with_alpha(ui.success, 0.8),
        GrowthPhase::Growing | GrowthPhase::FullGrowth => ui.success,
        GrowthPhase::Withering => ui.error,
        GrowthPhase::Blooming => ui.warning,
        GrowthPhase::Finished => [0.0; 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn growth_indicators_take_the_themes_status_colors() {
        let theme = crate::config::theme::Theme::load_builtin("solarized-light").unwrap();
        let ui = UiPalette::from_theme_with_contrast(&theme, false);
        assert_eq!(
            growth_color(&ui, GrowthPhase::Seed),
            with_alpha(theme.ui.foreground.to_f32_array(), 0.6)
        );
        assert_eq!(
            growth_color(&ui, GrowthPhase::Sprouting),
            with_alpha(ui.success, 0.8)
        );
        assert_eq!(growth_color(&ui, GrowthPhase::Growing), ui.success);
        assert_eq!(growth_color(&ui, GrowthPhase::FullGrowth), ui.success);
        assert_eq!(growth_color(&ui, GrowthPhase::Withering), ui.error);
        assert_eq!(growth_color(&ui, GrowthPhase::Blooming), ui.warning);
        assert_eq!(growth_color(&ui, GrowthPhase::Finished)[3], 0.0);
    }

    #[test]
    fn cellar_markers_say_how_each_command_fared_in_theme_colors() {
        let theme = crate::config::theme::Theme::load_builtin("solarized-light").unwrap();
        let ui = UiPalette::from_theme_with_contrast(&theme, false);
        assert_eq!(
            health_marker(&ui, CommandHealth::Healthy),
            ('\u{25CF}', ui.success)
        );
        assert_eq!(
            health_marker(&ui, CommandHealth::Wilted),
            ('\u{2716}', ui.error)
        );
        assert_eq!(
            health_marker(&ui, CommandHealth::Running),
            ('\u{25CB}', ui.menu_text_dim)
        );
    }
}
