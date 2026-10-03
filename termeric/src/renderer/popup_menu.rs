//! Anchored popup menus drawn in the desktop UI face.

use super::Renderer;
use crate::ui::color::scale_alpha;
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
    pub(super) fn build_popup_menu(&mut self, menu: &PopupMenuInfo) {
        let scale = menu.layout.scale;
        let px = |logical: f32| (logical * scale).round().max(1.0);
        let (x, y, width, height) = menu.layout.panel;
        let radius = px(8.0);
        for (spread, alpha) in [(px(2.0), 0.16), (px(8.0), 0.08)] {
            self.rect_pipeline.push_rounded_rect(
                x - spread,
                y - spread / 2.0 + px(2.0),
                width + spread * 2.0,
                height + spread * 2.0,
                [0.0, 0.0, 0.0, alpha],
                radius + spread,
            );
        }
        let line = px(1.0);
        self.rect_pipeline
            .push_rounded_rect(x, y, width, height, self.ui.menu_border, radius);
        self.rect_pipeline.push_rounded_rect(
            x + line,
            y + line,
            width - line * 2.0,
            height - line * 2.0,
            self.ui.menu_bg,
            radius - line,
        );

        let size = self.ui_label_px();
        let inset = px(4.0);
        let side = px(12.0);
        // Reserve a check column only when some item is checked, so labels
        // stay aligned without indenting menus that never show one.
        let check_column = menu
            .rows
            .iter()
            .any(|row| matches!(row, PopupMenuRowInfo::Item { checked: true, .. }));
        for (index, (row, &(row_x, row_y, row_width, row_height))) in
            menu.rows.iter().zip(&menu.layout.rows).enumerate()
        {
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
                            px(5.0),
                        );
                    }
                    let text = if *enabled {
                        self.ui.menu_text
                    } else {
                        scale_alpha(self.ui.menu_text_dim, 0.6)
                    };
                    let check_x = row_x + side;
                    if *checked {
                        let (cx, cy) = (check_x + px(6.0), row_y + row_height / 2.0);
                        let stroke = (1.5 * scale).max(1.0);
                        let unit = scale;
                        self.rect_pipeline.push_line(
                            cx - 4.0 * unit,
                            cy,
                            cx - 1.0 * unit,
                            cy + 3.0 * unit,
                            stroke,
                            text,
                        );
                        self.rect_pipeline.push_line(
                            cx - 1.0 * unit,
                            cy + 3.0 * unit,
                            cx + 4.5 * unit,
                            cy - 3.5 * unit,
                            stroke,
                            text,
                        );
                    }
                    let text_x = if check_column {
                        check_x + px(20.0)
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
