//! Assembling one frame's chrome: each surface's render info, built and its
//! animation stepped before the frame borrows the renderer.

use super::*;
use crate::renderer::{
    ContextPickerInfo, DiffReviewInfo, GuardrailInfo, InfoOverlayInfo, PopupMenuInfo,
};
use crate::ui::agents_panel::AgentsPanelInfo;
use crate::ui::command_palette::PaletteInfo;
use crate::ui::commands::panel::CommandsPanelInfo;
use crate::ui::search_bar::SearchBarInfo;
use crate::ui::settings::SettingsPanelInfo;
use crate::ui::shortcuts_panel::ShortcutsPanelInfo;

/// The chrome surfaces one frame draws, owned until the renderer borrows or
/// takes them. `None` surfaces are closed.
pub(super) struct ChromeInfo {
    pub(super) settings: Option<SettingsPanelInfo>,
    pub(super) search_bar: Option<SearchBarInfo>,
    pub(super) workspace_menu: Option<PopupMenuInfo>,
    pub(super) session_panel: Option<crate::ui::session_panel::SessionPanelInfo>,
    pub(super) commands_panel: Option<CommandsPanelInfo>,
    pub(super) agents_panel: Option<AgentsPanelInfo>,
    pub(super) palette: Option<PaletteInfo>,
    pub(super) shortcuts: Option<ShortcutsPanelInfo>,
    pub(super) info_overlay: Option<InfoOverlayInfo>,
    pub(super) toasts: Vec<crate::ui::toast::ToastInfo>,
    pub(super) diff_review: Option<DiffReviewInfo>,
    pub(super) context_picker: Option<ContextPickerInfo>,
    pub(super) guardrail: Option<GuardrailInfo>,
}

impl App {
    /// Build every chrome surface for this frame. Each surface steps its
    /// animation in the same place relative to its own build as it always
    /// has; the result says whether any of them needs another frame.
    pub(super) fn build_chrome(
        &mut self,
        metrics: &UiMetrics,
        reduced_motion: bool,
    ) -> (ChromeInfo, bool) {
        let mut animating = false;
        let workspace_menu = self.workspace_menu_render_info();
        let agents_panel = (!self.agents_panel.is_closed()).then(|| self.agents_panel_info());
        let settings = self.settings_frame_info(metrics);
        let search_bar =
            (self.search.is_active() || self.search.has_results() || self.search.is_searching())
                .then(|| self.search.build_info(self.search_cursor_pos));
        animating |= self.settings_panel.needs_redraw();
        animating |= self.session_panel.needs_redraw();
        let session_panel = self.session_panel_frame_info(metrics);
        animating |= self.commands_panel.needs_redraw() || self.agents_panel.needs_redraw();
        self.bell.update();
        animating |= self.bell.is_visual_active();
        animating |= self.update_harvest_animations(reduced_motion);
        let commands_panel = (!self.commands_panel.is_closed())
            .then(|| self.commands_panel.build_info(&self.config.commands));
        animating |= self.command_palette.update_animation();
        animating |= self.domain_picker.palette_mut().update_animation();
        animating |= self.unicode_picker.palette_mut().update_animation();
        let palette = self.palette_frame_info(metrics);
        animating |= self.shortcuts_panel.update_animation();
        let shortcuts = self.shortcuts_frame_info(metrics);
        let info_overlay = self.info_overlay_frame_info();
        animating |= self.info_overlay.update_animation();
        // Toast lifecycle: prune expired, then snapshot.
        animating |= self.toasts.update();
        let toasts = self.toasts.build_info();
        let diff_review = self.diff_review_frame_info();
        animating |= self.diff_review.update_animation();
        let context_picker = self.context_picker_frame_info();
        animating |= self.context_picker.update_animation();
        let guardrail = self.guardrail_frame_info();
        animating |= self.guardrail.update_animation();
        (
            ChromeInfo {
                settings,
                search_bar,
                workspace_menu,
                session_panel,
                commands_panel,
                agents_panel,
                palette,
                shortcuts,
                info_overlay,
                toasts,
                diff_review,
                context_picker,
                guardrail,
            },
            animating,
        )
    }

    fn settings_frame_info(&mut self, metrics: &UiMetrics) -> Option<SettingsPanelInfo> {
        if !self.settings_panel.is_visible() {
            return None;
        }
        // Tell the panel how many rows it can scroll through so its
        // scroll/selection logic stays in sync with the rendered viewport.
        let layout = crate::ui::settings::SettingsModalLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            self.settings_panel.animation_progress(),
            0,
        );
        self.settings_panel.set_max_visible(layout.rows_that_fit);
        let unsaved = self.config_has_unsaved_changes();
        let mut info = self
            .settings_panel
            .build_info(&self.config, &self.shortcut_registry);
        info.unsaved = unsaved;
        Some(info)
    }

    fn session_panel_frame_info(
        &mut self,
        metrics: &UiMetrics,
    ) -> Option<crate::ui::session_panel::SessionPanelInfo> {
        if self.session_panel.is_closed() {
            return None;
        }
        let preview = self.session_panel.build_info();
        let layout = crate::ui::session_panel::SessionPanelLayout::calculate(
            metrics.width,
            metrics.height,
            metrics.tab_bar_height,
            metrics.cell_width,
            metrics.cell_height,
            preview.animation,
            preview.rename_text.is_some(),
            preview.workspaces,
            preview.sessions.len(),
        );
        self.session_panel.set_max_visible(layout.rows_that_fit);
        Some(self.session_panel.build_info())
    }

    /// The palette-style modals share the palette's renderer and geometry;
    /// only the one that owns input is drawn.
    fn palette_frame_info(&mut self, metrics: &UiMetrics) -> Option<PaletteInfo> {
        let rows = crate::ui::list_dialog::ListDialogLayout::calculate(
            crate::ui::command_palette::PALETTE_DIALOG,
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            1.0,
            0,
        )
        .rows_that_fit;
        palette_input::open_palette_owner(
            &mut self.domain_picker,
            &mut self.unicode_picker,
            &mut self.command_palette,
        )
        .map(|owner| {
            owner.palette_mut().set_max_visible(rows);
            owner.build_info()
        })
    }

    fn shortcuts_frame_info(&mut self, metrics: &UiMetrics) -> Option<ShortcutsPanelInfo> {
        if !self.shortcuts_panel.is_visible() {
            return None;
        }
        let layout = crate::ui::list_dialog::ListDialogLayout::calculate(
            crate::ui::shortcuts_panel::SHORTCUTS_DIALOG,
            metrics.width,
            metrics.height,
            metrics.cell_width,
            metrics.cell_height,
            1.0,
            0,
        );
        self.shortcuts_panel.set_max_visible(layout.rows_that_fit);
        Some(self.shortcuts_panel.build_info(&self.shortcut_registry))
    }

    fn info_overlay_frame_info(&mut self) -> Option<InfoOverlayInfo> {
        if !self.info_overlay.is_visible() {
            return None;
        }
        let layout = self.info_overlay_layout();
        self.info_overlay
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        let scroll_offset = self.info_overlay.viewport().row();
        let horizontal_offset = self.info_overlay.viewport().column();
        let visible_lines = self
            .info_overlay
            .visible_lines()
            .map(|(source_line, line)| {
                (
                    source_line,
                    line.chars()
                        .skip(horizontal_offset)
                        .take(layout.columns_that_fit)
                        .collect(),
                )
            })
            .collect();
        let choices = self
            .info_overlay
            .choices()
            .iter()
            .filter(|choice| {
                choice.source_line >= scroll_offset
                    && choice.source_line < scroll_offset.saturating_add(layout.rows_that_fit)
            })
            .cloned()
            .collect();
        Some(InfoOverlayInfo {
            title: self.info_overlay.title().to_string(),
            visible_lines,
            choices,
            input: self.info_overlay.input_info(self.info_overlay.request_id()),
            focus: self.info_overlay.focus().cloned(),
            total_lines: self.info_overlay.lines().len(),
            scroll_offset,
            horizontal_offset,
            awaiting: self.info_overlay.is_awaiting_current(),
            animation: self.info_overlay.animation(),
            layout,
            mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
            mouse_pressed: self.mouse_pressed,
        })
    }

    fn diff_review_frame_info(&mut self) -> Option<DiffReviewInfo> {
        if !self.diff_review.is_visible() {
            return None;
        }
        let layout = self.diff_review_layout();
        self.diff_review
            .set_viewport(layout.rows_that_fit, layout.columns_that_fit);
        Some(DiffReviewInfo {
            filename: self.diff_review.filename().to_string(),
            visible_rows: self
                .diff_review
                .visible_rows()
                .map(|(source_row, row)| (source_row, row.clone()))
                .collect(),
            total_rows: self.diff_review.rows().len(),
            scroll_offset: self.diff_review.viewport().row(),
            horizontal_offset: self.diff_review.viewport().column(),
            awaiting: self.diff_review.is_awaiting_current(),
            animation: self.diff_review.animation(),
            layout,
            mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
            mouse_pressed: self.mouse_pressed,
        })
    }

    fn context_picker_frame_info(&mut self) -> Option<ContextPickerInfo> {
        if !self.context_picker.is_visible() {
            return None;
        }
        let layout = self.context_picker_layout();
        self.context_picker.set_viewport(layout.visible_count);
        Some(ContextPickerInfo {
            snapshot: self.context_picker.snapshot(),
            layout,
            mouse_pos: self.last_cursor_pos.map(|(x, y)| (x as f32, y as f32)),
            mouse_pressed: self.mouse_pressed,
        })
    }

    fn guardrail_frame_info(&self) -> Option<GuardrailInfo> {
        if !self.guardrail.is_visible() {
            return None;
        }
        self.guardrail.matched.as_ref().map(|m| GuardrailInfo {
            command: m.command.clone(),
            reason: m.reason.clone(),
            is_critical: m.severity == crate::terminal::guardrails::Severity::Critical,
            animation: self.guardrail.animation(),
            target: self.guardrail.target.clone(),
            layout: self.guardrail_layout(),
        })
    }

    /// The focused pane's Harvest growth indicators.
    fn harvest_indicators(&self) -> &[crate::harvest::GrowthIndicator] {
        self.tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(|tab| tab.focused_pane())
            .map(|pane| pane.harvest.indicators.as_slice())
            .unwrap_or_default()
    }

    /// The focused pane's growth indicators that are still shown, each
    /// beside its row of that pane as `placement` places it. An indicator
    /// follows its row as the pane scrolls back, and is left out while that
    /// row is scrolled out of view.
    pub(super) fn harvest_indicator_infos(
        &self,
        placement: crate::renderer::PanePlacement,
        cell_size: (f32, f32),
        reduced_motion: bool,
    ) -> Vec<IndicatorRenderInfo> {
        if !self.config.harvest.show_indicators {
            return Vec::new();
        }
        let Some(tab) = self.tabs.as_ref().and_then(TabManager::active_tab) else {
            return Vec::new();
        };
        let Some(rect) = tab
            .panes
            .calculate_layout()
            .get(&tab.panes.focused_id())
            .copied()
        else {
            return Vec::new();
        };
        let Some(pane) = tab.focused_pane() else {
            return Vec::new();
        };
        let lone = tab.panes.pane_count() == 1;
        // The grid the pane draws, so markers sit on the rows it shows.
        let grid = pane.terminal.presentation_grid();
        let scroll_offset = grid.scroll_offset();
        let (_, pane_rows) = crate::renderer::PanePlacement::pane_cells(
            rect,
            lone,
            (grid.cols(), grid.rows()),
            cell_size,
        );
        let shown_rows = pane_rows.min(grid.rows());
        pane.harvest
            .indicators
            .iter()
            .filter(|indicator| !indicator.is_finished())
            .filter_map(|indicator| {
                // Prompt rows count from the live screen's top, negative in
                // scrollback; scrolling back moves that screen down by the
                // scroll offset.
                let row = i64::from(indicator.prompt_row) + i64::try_from(scroll_offset).ok()?;
                let row = usize::try_from(row).ok()?;
                (row < shown_rows).then(|| IndicatorRenderInfo {
                    cell: placement.row_marker(rect, lone, row, cell_size),
                    phase: indicator.phase,
                    opacity: if reduced_motion {
                        1.0
                    } else {
                        indicator.opacity()
                    },
                })
            })
            .collect()
    }

    /// Step the Harvest cellar's animation; true while Harvest chrome still
    /// moves.
    fn update_harvest_animations(&mut self, reduced_motion: bool) -> bool {
        let has_active_indicator = !self.harvest_indicators().is_empty();
        let Some(harvest) = self.harvest.as_mut() else {
            return false;
        };
        let cellar_changed = harvest.cellar.update();
        if reduced_motion {
            cellar_changed
        } else {
            cellar_changed
                || harvest.cellar.is_visible()
                || harvest.sprout_menu.is_some()
                || has_active_indicator
        }
    }

    /// Build Harvest geometry once, then share it with painting, pointer
    /// hit-testing, and accessibility.
    pub(super) fn harvest_frame_info(&mut self, reduced_motion: bool) -> Option<HarvestRenderInfo> {
        let cellar_layout = self
            .harvest
            .as_ref()
            .is_some_and(|harvest| harvest.cellar.is_visible())
            .then(|| self.cellar_layout())
            .flatten();
        let sprout_menu_layout = self.sprout_menu_layout();
        let renderer = self.renderer.as_ref()?;
        let harvest = self.harvest.as_ref()?;
        let indicators = self.harvest_indicator_infos(
            renderer.pane_placement(),
            renderer.cell_size(),
            reduced_motion,
        );
        // Cellar panel (visible while open or animating out)
        let cellar = cellar_layout.map(|layout| CellarRenderInfo {
            entries: harvest
                .cellar
                .visible_entries()
                .map(|e| CellarEntryInfo {
                    id: e.id,
                    command: e.command.clone(),
                    exit_code: e.exit_code,
                    output_truncated: e.output_truncated,
                    health: e.health(),
                })
                .collect(),
            total_entries: harvest.cellar.len(),
            evicted_entries: harvest.cellar.evicted_commands(),
            animation_progress: harvest.cellar.animation.progress(),
            success_rate: harvest.cellar.success_rate(),
            scroll: harvest.cellar.scroll,
            selected_id: harvest.cellar.selected_id(),
            layout,
        });
        let sprout_menu = harvest
            .sprout_menu
            .as_ref()
            .zip(sprout_menu_layout)
            .map(|(menu, layout)| sprout_menu_render_info(menu, layout));
        Some(HarvestRenderInfo {
            indicators,
            cellar,
            sprout_menu,
        })
    }
}
