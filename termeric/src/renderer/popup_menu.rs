//! Anchored popup menus drawn in the desktop UI face.

use super::Renderer;
use crate::ui::color::scale_alpha;
use crate::ui::constants::{BUTTON_CORNER_RADIUS, MENU_CORNER_RADIUS};
use crate::ui::geometry::Bounds;
use crate::ui::icon::Icon;
use crate::ui::popup_menu::{MenuRow, PopupMenuLayout};

/// A menu row as drawn, independent of the action it triggers.
#[derive(Clone, Debug, PartialEq)]
pub enum PopupMenuRowInfo {
    Item {
        label: String,
        detail: Option<String>,
        checked: bool,
        enabled: bool,
    },
    Separator,
}

/// Everything needed to draw one open menu.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupMenuInfo {
    pub layout: PopupMenuLayout,
    pub rows: Vec<PopupMenuRowInfo>,
    pub highlighted: Option<usize>,
}

impl PopupMenuInfo {
    pub fn new<A>(
        rows: &[MenuRow<A>],
        highlighted: Option<usize>,
        layout: PopupMenuLayout,
    ) -> Self {
        let rows = rows
            .iter()
            .map(|row| match row {
                MenuRow::Item(item) => PopupMenuRowInfo::Item {
                    label: item.label.clone(),
                    detail: item.detail.clone(),
                    checked: item.checked,
                    enabled: item.enabled,
                },
                MenuRow::Separator => PopupMenuRowInfo::Separator,
            })
            .collect();
        Self {
            layout,
            rows,
            highlighted,
        }
    }
}

impl Renderer {
    /// The surface every anchored menu sits on (popup menus and the Settings
    /// dropdown): a soft two-layer shadow, a hairline border, and the menu
    /// background, all faded by `opacity`.
    pub(super) fn push_menu_surface(&mut self, bounds: Bounds, opacity: f32) {
        let Bounds {
            x,
            y,
            width,
            height,
        } = bounds;
        let radius = self.px(MENU_CORNER_RADIUS);
        for (spread, alpha) in [(self.px(2.0), 0.16), (self.px(8.0), 0.08)] {
            self.rect_pipeline.push_rounded_rect(
                x - spread,
                y - spread / 2.0 + self.px(2.0),
                width + spread * 2.0,
                height + spread * 2.0,
                [0.0, 0.0, 0.0, alpha * opacity],
                radius + spread,
            );
        }
        let line = self.px(1.0);
        self.rect_pipeline.push_rounded_rect(
            x,
            y,
            width,
            height,
            scale_alpha(self.ui.menu_border, opacity),
            radius,
        );
        self.rect_pipeline.push_rounded_rect(
            x + line,
            y + line,
            width - line * 2.0,
            height - line * 2.0,
            scale_alpha(self.ui.menu_bg, opacity),
            radius - line,
        );
    }

    pub(super) fn build_popup_menu(&mut self, menu: &PopupMenuInfo) {
        self.push_menu_surface(menu.layout.panel, 1.0);
        let line = self.px(1.0);

        let size = self.ui_label_px();
        let inset = self.px(4.0);
        let side = self.px(12.0);
        // Reserve a check column only when some item is checked, so labels
        // stay aligned without indenting menus that never show one.
        let check_column = menu
            .rows
            .iter()
            .any(|row| matches!(row, PopupMenuRowInfo::Item { checked: true, .. }));
        for (index, (row, bounds)) in menu.rows.iter().zip(&menu.layout.rows).enumerate() {
            let Bounds {
                x: row_x,
                y: row_y,
                width: row_width,
                height: row_height,
            } = *bounds;
            match row {
                PopupMenuRowInfo::Separator => {
                    self.rect_pipeline.push_rect(
                        row_x + side,
                        (row_y + row_height / 2.0).round(),
                        row_width - side * 2.0,
                        line,
                        self.ui.menu_border,
                    );
                }
                PopupMenuRowInfo::Item {
                    label,
                    detail,
                    checked,
                    enabled,
                } => {
                    if menu.highlighted == Some(index) {
                        self.rect_pipeline.push_rounded_rect(
                            row_x + inset,
                            row_y,
                            row_width - inset * 2.0,
                            row_height,
                            self.ui.menu_highlight,
                            self.px(BUTTON_CORNER_RADIUS),
                        );
                    }
                    let text = if *enabled {
                        self.ui.menu_text
                    } else {
                        scale_alpha(self.ui.menu_text_dim, 0.6)
                    };
                    let check_x = row_x + side;
                    if *checked {
                        self.push_icon(
                            Icon::Check,
                            check_x + self.px(6.0),
                            row_y + row_height / 2.0,
                            text,
                        );
                    }
                    let text_x = if check_column {
                        check_x + self.px(20.0)
                    } else {
                        check_x
                    };
                    let right = row_x + row_width - side;
                    let detail_width = match detail {
                        Some(detail) => {
                            let width = self.ui_text_width(detail, size);
                            self.push_ui_label(
                                detail,
                                right - width,
                                row_y,
                                row_height,
                                width + 1.0,
                                size,
                                self.ui.menu_text_dim,
                            );
                            width + side
                        }
                        None => 0.0,
                    };
                    self.push_ui_label(
                        label,
                        text_x,
                        row_y,
                        row_height,
                        right - detail_width - text_x,
                        size,
                        text,
                    );
                }
            }
        }
    }
}
