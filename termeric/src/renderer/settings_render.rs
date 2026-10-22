//! Settings dialog rendering — header with search, page sidebar, rows, and a
//! footer with Save.

use super::Renderer;
use super::draw::{ButtonState, FieldTextColors};
use crate::ui::color::scale_alpha;
use crate::ui::constants::*;
use crate::ui::dialog::ButtonRole;
use crate::ui::geometry::Bounds;
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
        layout: &SettingsModalLayout,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }
        let cell_width = layout.cell_width;
        let cell_height = layout.cell_height;
        let fade = |color: [f32; 4], alpha: f32| scale_alpha(color, alpha * anim);

        let scrim = fade(self.ui.palette_scrim, 1.0);
        self.rect_pipeline
            .push_rect(0.0, 0.0, window_width, window_height, scrim);

        let Bounds {
            x: panel_x,
            y: panel_y,
            width: panel_width,
            height: panel_height,
        } = layout.panel;
        self.build_modal_frame(panel_x, panel_y, panel_width, panel_height, anim);
        let line = self.px(1.0);
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
        self.draw_filter_field(input, field, dim);
        if let Some(view) = layout.input_viewport(&info.filter, info.filter_cursor) {
            self.draw_field_text(
                input,
                &view,
                &info.filter,
                "Search settings",
                FieldTextColors {
                    text,
                    placeholder: fade(self.ui.palette_text_dim, 0.6),
                },
                (!info.dropdown_open).then(|| fade(self.ui.palette_accent, 0.9)),
            );
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
                    self.px(BUTTON_CORNER_RADIUS),
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
        } else if let Some(title) = &info.sub_page_title {
            title.clone()
        } else {
            info.page.label().to_string()
        };
        let back = info.sub_page_title.is_some();
        if back {
            self.build_header_icon_button(
                layout.back_button,
                Icon::Back,
                info.hovered_control == Some(SettingsControl::Back),
            );
        }
        let heading_x = layout.heading_x(back);
        self.push_ui_label(
            &heading,
            heading_x,
            layout.content_title_y,
            cell_height * SettingsModalLayout::HEADING_LINES,
            layout.content_x + layout.content_width - heading_x,
            size * 1.1,
            if searching { dim } else { text },
        );

        let first_selectable = info.entries[..info.scroll_offset.min(info.entries.len())]
            .iter()
            .filter(|entry| !entry.is_header)
            .count();
        let mut selectable_index = first_selectable;
        let end = (info.scroll_offset + layout.rows_that_fit).min(info.entries.len());
        let value_right = layout.content_x + layout.content_width;
        for (row_index, entry) in info.entries[info.scroll_offset.min(end)..end]
            .iter()
            .enumerate()
        {
            let row = layout.row_rect(row_index);
            if entry.is_header {
                self.push_ui_label(
                    &entry.label,
                    layout.content_x,
                    row.y,
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
            if let SettingKind::Button { role, .. } = &entry.kind {
                // The button is the row: it shows hover and keyboard focus.
                self.draw_button(
                    layout.row_button(row_index),
                    &entry.label,
                    *role,
                    ButtonState {
                        hovered,
                        focused: selected,
                        ..ButtonState::default()
                    },
                    anim,
                );
                continue;
            }
            if selected || hovered {
                self.rect_pipeline.push_rounded_rect(
                    row.x,
                    row.y,
                    row.width,
                    row.height,
                    if selected {
                        highlight
                    } else {
                        fade(self.ui.menu_highlight, 0.55)
                    },
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }
            let label_width = layout.value_x - layout.content_x - padding;
            let drawn = self.push_ui_label(
                &entry.label,
                layout.content_x,
                row.y,
                row.height,
                label_width,
                size,
                text,
            );
            if let Some(note) = &entry.note {
                let gap = self.px(8.0);
                self.push_ui_label(
                    note,
                    layout.content_x + drawn + gap,
                    row.y,
                    row.height,
                    (label_width - drawn - gap).max(0.0),
                    size,
                    dim,
                );
            }
            let value_width = value_right - layout.value_x;
            let center_y = row.y + row.height / 2.0;
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
                            self.px(BUTTON_CORNER_RADIUS),
                        );
                        self.push_icon(icon, x + button / 2.0, center_y, text);
                    }
                    let box_width = layout.stepper_value_width();
                    self.push_ui_label_centered(
                        &entry.value,
                        layout.value_text_x() + box_width / 2.0,
                        row.y,
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
                        row.y,
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
                        row.y,
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
                SettingKind::SwitchedAction { on, .. } => {
                    let switch = layout.row_switch(row_index);
                    self.push_ui_label(
                        &entry.value,
                        layout.value_x,
                        row.y,
                        row.height,
                        (switch.x - layout.value_x - cell_width).max(0.0),
                        size,
                        dim,
                    );
                    self.draw_switch(switch, *on, anim);
                    self.push_icon(Icon::ChevronRight, value_right - cell_width, center_y, dim);
                }
                // Drawn above, as the whole row.
                SettingKind::Button { .. } => {}
                SettingKind::ReadOnly => {
                    self.push_ui_label(
                        &entry.value,
                        layout.value_x,
                        row.y,
                        row.height,
                        value_width,
                        size,
                        dim,
                    );
                }
            }
        }

        // ── Scrollbar: a thin thumb only while rows overflow ──
        let scrollbar_width = self.px(layout.scrollbar_width);
        self.draw_scroll_thumb(
            Bounds::new(
                panel_x + panel_width - padding * 0.5 - scrollbar_width,
                layout.list_y,
                scrollbar_width,
                layout.rows_that_fit as f32 * layout.row_height,
            ),
            layout.rows_that_fit,
            info.entries.len(),
            info.scroll_offset,
            fade(self.ui.menu_text_dim, 0.6),
        );

        // ── Footer: save status, Discard… while unsaved, and Save ──
        let footer_y = layout.footer_y.round();
        self.rect_pipeline.push_rect(
            panel_x + line,
            footer_y,
            panel_width - line * 2.0,
            line,
            border,
        );
        let status = if info.unsaved {
            "Unsaved changes"
        } else {
            "Changes apply immediately"
        };
        self.push_ui_label(
            status,
            panel_x + padding,
            footer_y,
            layout.footer_height,
            if info.unsaved {
                layout.discard_button.x
            } else {
                layout.save_button.x
            } - panel_x
                - padding * 2.0,
            size * 0.9,
            if info.unsaved {
                fade(self.ui.palette_accent, 1.0)
            } else {
                dim
            },
        );
        if info.unsaved {
            self.draw_button(
                layout.discard_button,
                "Discard…",
                ButtonRole::Destructive,
                ButtonState {
                    hovered: info.hovered_control == Some(SettingsControl::Discard),
                    ..ButtonState::default()
                },
                anim,
            );
        }
        self.draw_button(
            layout.save_button,
            "Save",
            ButtonRole::Suggested,
            ButtonState {
                hovered: info.hovered_control == Some(SettingsControl::Save),
                ..ButtonState::default()
            },
            anim,
        );
    }

    /// Build dropdown overlay render data for settings — rendered last (on top)
    pub(super) fn build_dropdown_overlay(
        &mut self,
        info: &SettingsPanelInfo,
        layout: &SettingsModalLayout,
    ) {
        if !info.dropdown_open {
            return;
        }
        let anim = info.animation;
        if anim <= 0.0 {
            return;
        }

        let cell_width = self.cell_width();

        // The same dropdown the panel hit-tests clicks and hover against.
        let Some(dropdown) = info.dropdown_layout(layout) else {
            return;
        };
        let rect = dropdown.rect;

        let fade = |color: [f32; 4]| scale_alpha(color, anim);
        self.push_menu_surface(rect.bounds, anim);

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
                    self.px(BUTTON_CORNER_RADIUS),
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
