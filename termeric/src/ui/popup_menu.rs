//! Anchored popup menus: the row model, keyboard navigation, and one
//! whole-pixel layout shared by rendering, pointer hit testing, and
//! accessibility bounds.

/// Pixel rectangle: x, y, width, height.
pub type Rect = (f32, f32, f32, f32);

/// Logical metrics before display scaling.
const MENU_WIDTH: f32 = 280.0;
const ROW_HEIGHT: f32 = 30.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const PADDING: f32 = 6.0;
const WINDOW_MARGIN: f32 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem<A> {
    pub label: String,
    /// Secondary text drawn right-aligned and dimmed, such as a count.
    pub detail: Option<String>,
    pub checked: bool,
    pub enabled: bool,
    pub action: A,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuRow<A> {
    Item(MenuItem<A>),
    Separator,
}

impl<A> MenuRow<A> {
    fn is_selectable(&self) -> bool {
        matches!(self, MenuRow::Item(item) if item.enabled)
    }
}

/// An open menu. Only enabled items can be highlighted.
#[derive(Clone, Debug)]
pub struct PopupMenu<A> {
    rows: Vec<MenuRow<A>>,
    highlighted: Option<usize>,
}

impl<A: Clone> PopupMenu<A> {
    /// A menu opened from the keyboard starts on its first item so arrow
    /// keys and Enter work immediately; a pointer-opened menu starts
    /// without a highlight until the pointer moves over a row.
    pub fn new(rows: Vec<MenuRow<A>>, highlight_first: bool) -> Self {
        let mut menu = Self {
            rows,
            highlighted: None,
        };
        if highlight_first {
            menu.highlight_edge(false);
        }
        menu
    }

    pub fn rows(&self) -> &[MenuRow<A>] {
        &self.rows
    }

    pub fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// Highlight `row` when it is an enabled item; anything else clears it.
    pub fn set_highlight(&mut self, row: Option<usize>) {
        self.highlighted =
            row.filter(|&row| self.rows.get(row).is_some_and(MenuRow::is_selectable));
    }

    /// Move to the next (`forward`) or previous enabled item, wrapping.
    pub fn move_highlight(&mut self, forward: bool) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let start = match (self.highlighted, forward) {
            (Some(row), _) => row,
            (None, true) => count - 1,
            (None, false) => 0,
        };
        for step in 1..=count {
            let row = if forward {
                (start + step) % count
            } else {
                (start + count - step % count) % count
            };
            if self.rows[row].is_selectable() {
                self.highlighted = Some(row);
                return;
            }
        }
    }

    /// Highlight the first or last enabled item.
    pub fn highlight_edge(&mut self, last: bool) {
        let mut rows = self.rows.iter().enumerate();
        let found = if last {
            rows.rev().find(|(_, row)| row.is_selectable())
        } else {
            rows.find(|(_, row)| row.is_selectable())
        };
        self.highlighted = found.map(|(index, _)| index);
    }

    /// The action of `row` when it is an enabled item.
    pub fn action_at(&self, row: usize) -> Option<A> {
        match self.rows.get(row)? {
            MenuRow::Item(item) if item.enabled => Some(item.action.clone()),
            _ => None,
        }
    }

    pub fn highlighted_action(&self) -> Option<A> {
        self.highlighted.and_then(|row| self.action_at(row))
    }
}

/// Whole-pixel geometry of an open menu.
#[derive(Clone, Debug, PartialEq)]
pub struct PopupMenuLayout {
    pub panel: Rect,
    /// One rectangle per row, separators included, in row order.
    pub rows: Vec<Rect>,
    /// Display scale the layout was computed for.
    pub scale: f32,
}

impl PopupMenuLayout {
    /// Place a menu whose right edge aligns with `anchor_right`, opening
    /// downward from `top`, kept inside the window.
    pub fn calculate<A>(
        rows: &[MenuRow<A>],
        anchor_right: f32,
        top: f32,
        window_width: f32,
        scale: f32,
    ) -> Self {
        let scale = scale.max(0.1);
        let px = |logical: f32| (logical * scale).round();
        let margin = px(WINDOW_MARGIN);
        let width = px(MENU_WIDTH).min((window_width - margin * 2.0).max(0.0));
        let x = (anchor_right.round() - width)
            .min(window_width - margin - width)
            .max(margin);
        let y = top.round() + px(2.0);
        let padding = px(PADDING);
        let mut row_y = y + padding;
        let rects = rows
            .iter()
            .map(|row| {
                let height = match row {
                    MenuRow::Item(_) => px(ROW_HEIGHT),
                    MenuRow::Separator => px(SEPARATOR_HEIGHT),
                };
                let rect = (x, row_y, width, height);
                row_y += height;
                rect
            })
            .collect();
        Self {
            panel: (x, y, width, row_y + padding - y),
            rows: rects,
            scale,
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        contains(self.panel, x, y)
    }

    pub fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rows.iter().position(|&rect| contains(rect, x, y))
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.0 && x < rect.0 + rect.2 && y >= rect.1 && y < rect.1 + rect.3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, enabled: bool) -> MenuRow<&'static str> {
        MenuRow::Item(MenuItem {
            label: label.into(),
            detail: None,
            checked: false,
            enabled,
            action: match label {
                "a" => "a",
                "b" => "b",
                "c" => "c",
                _ => "other",
            },
        })
    }

    fn rows() -> Vec<MenuRow<&'static str>> {
        vec![
            item("a", true),
            item("b", false),
            MenuRow::Separator,
            item("c", true),
        ]
    }

    #[test]
    fn keyboard_opening_highlights_the_first_enabled_item() {
        assert_eq!(PopupMenu::new(rows(), true).highlighted(), Some(0));
        assert_eq!(PopupMenu::new(rows(), false).highlighted(), None);
        let disabled_first = vec![item("b", false), item("c", true)];
        assert_eq!(PopupMenu::new(disabled_first, true).highlighted(), Some(1));
    }

    #[test]
    fn navigation_skips_separators_and_disabled_items_and_wraps() {
        let mut menu = PopupMenu::new(rows(), true);
        menu.move_highlight(true);
        assert_eq!(menu.highlighted(), Some(3));
        menu.move_highlight(true);
        assert_eq!(menu.highlighted(), Some(0));
        menu.move_highlight(false);
        assert_eq!(menu.highlighted(), Some(3));
        menu.highlight_edge(false);
        assert_eq!(menu.highlighted_action(), Some("a"));
        menu.highlight_edge(true);
        assert_eq!(menu.highlighted_action(), Some("c"));

        let mut pointer = PopupMenu::new(rows(), false);
        pointer.move_highlight(true);
        assert_eq!(pointer.highlighted(), Some(0));
        let mut pointer = PopupMenu::new(rows(), false);
        pointer.move_highlight(false);
        assert_eq!(pointer.highlighted(), Some(3));
    }

    #[test]
    fn disabled_rows_and_separators_never_highlight_or_act() {
        let mut menu = PopupMenu::new(rows(), false);
        menu.set_highlight(Some(1));
        assert_eq!(menu.highlighted(), None);
        menu.set_highlight(Some(2));
        assert_eq!(menu.highlighted(), None);
        assert_eq!(menu.action_at(1), None);
        assert_eq!(menu.action_at(2), None);
        assert_eq!(menu.action_at(9), None);
        assert_eq!(menu.action_at(3), Some("c"));
    }

    #[test]
    fn layout_is_whole_pixel_right_aligned_and_inside_the_window() {
        for scale in [1.0, 1.25, 2.0] {
            let layout = PopupMenuLayout::calculate(&rows(), 700.0, 34.0, 800.0, scale);
            let (x, y, width, height) = layout.panel;
            assert_eq!(x + width, 700.0);
            for value in [x, y, width, height] {
                assert_eq!(value.fract(), 0.0);
            }
            assert_eq!(layout.rows.len(), 4);
            let last = layout.rows[3];
            assert!(last.1 + last.3 < y + height);
            let (row_x, row_y, _, row_h) = layout.rows[3];
            assert_eq!(layout.row_at(row_x + 5.0, row_y + row_h / 2.0), Some(3));
        }
        let narrow = PopupMenuLayout::calculate(&rows(), 100.0, 34.0, 200.0, 1.0);
        assert!(narrow.panel.0 >= 4.0 && narrow.panel.0 + narrow.panel.2 <= 196.0);
    }
}
