//! Settings dialog rendering — header with search, page sidebar, rows, and a
//! footer with Save.

use super::Renderer;
use crate::ui::color::scale_alpha;
use crate::ui::constants::*;
use crate::ui::icon::Icon;
use crate::ui::settings::{
    SettingKind, SettingsControl, SettingsModalLayout, SettingsPage, SettingsPanelInfo,
};

impl Renderer {
    /// Build the settings dialog: header with title, filter, and close; a
    /// sidebar of pages; the page's rows; and a footer with Save. Geometry
    /// comes from `SettingsModalLayout`, shared with input and accessibility.
    pub(super) fn build_settings_panel(
        &mut self,
        info: &SettingsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();
        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let fade = |color: [f32; 4], alpha: f32| scale_alpha(color, alpha * anim);

        let scrim = fade(self.ui.palette_scrim, 1.0);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let layout = SettingsModalLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );
        let (panel_x, panel_y) = (layout.panel_x.round(), layout.panel_y.round());
        let (panel_width, panel_height) = (layout.panel_width.round(), layout.panel_height.round());
        self.build_modal_frame(panel_x, panel_y, panel_width, panel_height, anim);
        let line = px(1.0);
        let size = self.ui_label_px();
        let text = fade(self.ui.palette_text, 1.0);
        let dim = fade(self.ui.menu_text_dim, 1.0);
        let border = fade(self.ui.menu_border, 1.0);
        let highlight = fade(self.ui.menu_highlight, 1.0);
        let padding = layout.padding;

        // ── Header: title, filter field, close ──
        self.push_ui_label(
            "Settings",
            panel_x + padding,
            panel_y,
            layout.header_height,
            layout.input.x - panel_x - padding * 2.0,
            size * 1.15,
            text,
        );
        let input = layout.input;
        let field = fade(self.ui.palette_input_bg, 0.8);
        self.rect_pipeline.push_rounded_rect(
            input.x,
            input.y,
            input.width,
            input.height,
            field,
            px(6.0),
        );
        self.push_magnifier(
            input.x + cell_width * FILTER_ICON_CENTER_CELLS,
            input.y + input.height / 2.0,
            dim,
            field,
        );
        if let Some(view) = layout.input_viewport(&info.filter, info.filter_cursor, cell_height) {
            let clip = super::text::TextClip {
                left: view.text_x,
                top: input.y,
                right: view.right,
                bottom: input.y + input.height,
            };
            if info.filter.is_empty() {
                let hint = "Search settings";
                if let Some(hint_view) = layout.input_viewport(hint, 0, cell_height) {
                    self.text_pipeline.push_ui_run(
                        &hint[hint_view.text_range],
                        view.text_x,
                        view.text_y,
                        fade(self.ui.palette_text_dim, 0.6),
                        cell_width,
                        clip,
                    );
                }
            } else {
                self.text_pipeline.push_ui_run(
                    &info.filter[view.text_range.clone()],
                    view.text_x,
                    view.text_y,
                    text,
                    cell_width,
                    clip,
                );
            }
            if !info.dropdown_open {
                self.rect_pipeline.push_rect(
                    view.cursor_x,
                    view.text_y,
                    px(2.0).min(view.right - view.cursor_x),
                    cell_height,
                    fade(self.ui.palette_accent, 0.9),
                );
            }
        }
        self.build_header_icon_button(
            layout.close_button,
            Icon::Close,
            info.hovered_control == Some(SettingsControl::Close),
        );
        let body_top = (panel_y + layout.header_height).round();
        self.rect_pipeline.push_rect(
            panel_x + line,
            body_top,
            panel_width - line * 2.0,
            line,
            border,
        );

        // ── Sidebar ──
        let sidebar = layout.sidebar;
        self.rect_pipeline.push_rect(
            (sidebar.x + sidebar.width).round(),
            body_top,
            line,
            layout.footer_y - body_top,
            border,
        );
        let searching = !info.filter.is_empty();
        for (index, page) in SettingsPage::all().iter().enumerate() {
            let row = layout.page_rect(index);
            let current = *page == info.page;
            let hovered = info.hovered_control == Some(SettingsControl::Page(*page));
            if current || hovered {
                let fill = if current && !searching {
                    highlight
                } else {
                    fade(self.ui.menu_highlight, 0.55)
                };
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
                    fill,
                    px(6.0),
                );
            }
            self.push_ui_label(
                page.label(),
                row.x + padding * 0.5,
                row.y,
                row.height,
                row.width - padding,
                size,
                if current {
                    text
                } else {
                    fade(self.ui.palette_text, 0.8)
                },
            );
        }

        // ── Content: page title, then rows ──
        let heading = if searching {
            match info.entries.iter().filter(|entry| !entry.is_header).count() {
                0 => "No matching settings".to_string(),
                1 => "1 result".to_string(),
                count => format!("{count} results"),
            }
        } else {
            info.page.label().to_string()
        };
        self.push_ui_label(
            &heading,
            layout.content_x,
            layout.content_title_y,
            cell_height * 2.0,
            layout.content_width,
            size * 1.1,
            if searching { dim } else { text },
        );

        let first_selectable = info.entries[..info.scroll_offset.min(info.entries.len())]
            .iter()
            .filter(|entry| !entry.is_header)
            .count();
        let mut selectable_index = first_selectable;
        let end = (info.scroll_offset + layout.visible_count).min(info.entries.len());
        let value_right = layout.content_x + layout.content_width;
        for (row_index, entry) in info.entries[info.scroll_offset.min(end)..end]
            .iter()
            .enumerate()
        {
            let row = layout.row_rect(row_index);
            let row_y = row.y.round();
            if entry.is_header {
                self.push_ui_label(
                    entry.label,
                    layout.content_x,
                    row_y,
                    row.height,
                    layout.content_width,
                    size * 0.9,
                    dim,
                );
                continue;
            }
            let selected = selectable_index == info.selected;
            let hovered = info.hovered_item == Some(row_index);
            selectable_index += 1;
            if selected || hovered {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row_y,
                    row.width,
                    row.height,
                    if selected {
                        highlight
                    } else {
                        fade(self.ui.menu_highlight, 0.55)
                    },
                    px(6.0),
                );
            }
            self.push_ui_label(
                entry.label,
                layout.content_x,
                row_y,
                row.height,
                layout.value_x - layout.content_x - padding,
                size,
                text,
            );
            let value_width = value_right - layout.value_x;
            let center_y = row_y + row.height / 2.0;
            match &entry.kind {
                SettingKind::Numeric { .. } => {
                    let button = layout.value_text_x() - layout.numeric_decrement_x();
                    let control = cell_height * 1.4;
                    for (x, icon) in [
                        (layout.numeric_decrement_x(), Icon::Minus),
                        (layout.numeric_increment_x(), Icon::Plus),
                    ] {
                        self.rect_pipeline.push_rounded_rect(
                            x + (button - control) / 2.0,
                            center_y - control / 2.0,
                            control,
                            control,
                            field,
                            px(5.0),
                        );
                        self.push_icon(icon, x + button / 2.0, center_y, text);
                    }
                    let box_width = layout.stepper_value_width();
                    let width = self.ui_text_width(&entry.value, size).min(box_width);
                    self.push_ui_label(
                        &entry.value,
                        layout.value_text_x() + (box_width - width) / 2.0,
                        row_y,
                        row.height,
                        box_width,
                        size,
                        text,
                    );
                }
                SettingKind::Dropdown { .. } | SettingKind::SteppedList { .. } => {
                    let chevron = cell_width * 2.0;
                    let drawn = self.push_ui_label(
                        &entry.value,
                        layout.value_x,
                        row_y,
                        row.height,
                        value_width - chevron,
                        size,
                        text,
                    );
                    self.push_icon(
                        Icon::ChevronDown,
                        layout.value_x + drawn + chevron / 2.0,
                        center_y,
                        dim,
                    );
                }
                SettingKind::Action { .. } => {
                    let chevron = cell_width * 2.0;
                    let drawn = self.push_ui_label(
                        &entry.value,
                        layout.value_x,
                        row_y,
                        row.height,
                        value_width - chevron,
                        size,
                        dim,
                    );
                    self.push_icon(
                        Icon::ChevronRight,
                        layout.value_x + drawn + chevron / 2.0,
                        center_y,
                        dim,
                    );
                }
                SettingKind::ReadOnly => {
                    self.push_ui_label(
                        &entry.value,
                        layout.value_x,
                        row_y,
                        row.height,
                        value_width,
                        size,
                        dim,
                    );
                }
            }
        }

        // ── Scrollbar: a thin thumb only while rows overflow ──
        let total = info.entries.len();
        let rows = layout.rows_that_fit;
        if total > rows && rows > 0 {
            let track_h = rows as f32 * layout.row_height;
            let thumb_h = (track_h * rows as f32 / total as f32).max(px(24.0));
            let max_scroll = total - rows;
            let fraction = info.scroll_offset.min(max_scroll) as f32 / max_scroll as f32;
            let width = px(layout.scrollbar_width);
            self.rect_pipeline.push_rounded_rect(
                panel_x + panel_width - padding * 0.5 - width,
                layout.list_y + fraction * (track_h - thumb_h),
                width,
                thumb_h,
                fade(self.ui.menu_text_dim, 0.6),
                width / 2.0,
            );
        }

        // ── Footer: save status and Save ──
        let footer_y = layout.footer_y.round();
        self.rect_pipeline.push_rect(
            panel_x + line,
            footer_y,
            panel_width - line * 2.0,
            line,
            border,
        );
        let status = if info.unsaved {
            "Unsaved changes · Ctrl+S saves"
        } else {
            "Changes apply immediately"
        };
        self.push_ui_label(
            status,
            panel_x + padding,
            footer_y,
            layout.footer_height,
            layout.save_button.x - panel_x - padding * 2.0,
            size * 0.9,
            if info.unsaved {
                fade(self.ui.palette_accent, 1.0)
            } else {
                dim
            },
        );
        let save = layout.save_button;
        let save_hovered = info.hovered_control == Some(SettingsControl::Save);
        self.rect_pipeline.push_rounded_rect(
            save.x,
            save.y,
            save.width,
            save.height,
            if save_hovered { highlight } else { border },
            px(5.0),
        );
        let label_width = self.ui_text_width("Save", size);
        self.push_ui_label(
            "Save",
            save.x + (save.width - label_width) / 2.0,
            save.y,
            save.height,
            save.width,
            size,
            text,
        );
    }

    /// Build dropdown overlay render data for settings — rendered last (on top)
    pub(super) fn build_dropdown_overlay(
        &mut self,
        info: &SettingsPanelInfo,
        window_width: f32,
        window_height: f32,
    ) {
        if !info.dropdown_open {
            return;
        }
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let cell_width = self.cell_width();
        let cell_height = self.cell_height();

        let layout = SettingsModalLayout::calculate(
            window_width,
            window_height,
            cell_width,
            cell_height,
            anim,
            info.entries.len(),
        );

        // The same dropdown the panel hit-tests clicks and hover against.
        let Some(dropdown) = info.dropdown_layout(&layout) else {
            return;
        };
        let rect = dropdown.rect;

        let scale = self.scale_factor;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let fade = |color: [f32; 4]| scale_alpha(color, anim);
        self.push_menu_surface(rect.bounds, scale, anim);

        // ── Options in the UI face ──
        let size = self.ui_label_px();
        for (visible_index, (index, option)) in dropdown
            .options
            .iter()
            .enumerate()
            .skip(rect.first_option)
            .take(rect.visible_options)
            .enumerate()
        {
            let row = rect.option_bounds(visible_index);
            let highlighted = index == info.dropdown_highlight;
            if highlighted || info.dropdown_hovered == Some(index) {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
                    if highlighted {
                        fade(self.ui.menu_highlight)
                    } else {
                        scale_alpha(self.ui.menu_highlight, 0.55 * anim)
                    },
                    px(5.0),
                );
            }
            self.push_ui_label(
                option,
                rect.bounds.x + cell_width * 1.2,
                row.y,
                row.height,
                rect.bounds.width - cell_width * 2.4,
                size,
                fade(self.ui.menu_text),
            );
        }
    }
}
