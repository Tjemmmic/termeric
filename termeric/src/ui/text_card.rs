//! What the scrolling text cards (the info overlay and diff review) share:
//! the card geometry around their content and the two-axis viewport over it.

use super::constants::CONTENT_CHAR_SCALE;
use super::geometry::Bounds;
use super::scroll::reveal_row;

/// Geometry of a centred card showing scrolled monospace text under a title
/// and above a row of buttons. Rendering, hit testing, viewport sizing, and
/// accessibility all read it, so what is drawn is what answers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextCard {
    pub card: Bounds,
    pub content: Bounds,
    pub padding: f32,
    pub title_height: f32,
    pub line_height: f32,
    /// Horizontal advance of one content column.
    pub char_scale: f32,
    pub footer_height: f32,
    pub rows_that_fit: usize,
    pub columns_that_fit: usize,
    /// Top of the button row under the content.
    pub button_y: f32,
    pub button_height: f32,
    pub button_gap: f32,
}

impl TextCard {
    /// Lay out a card for `total_rows` rows of content, sized to show
    /// `content_columns` columns and a title `title_columns` long where the
    /// window allows, and never narrower than `min_width`. `animation`
    /// slides the card down into place as it opens.
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
        content_columns: usize,
        title_columns: usize,
        min_width: f32,
        total_rows: usize,
    ) -> Self {
        let char_scale = (cell_width * CONTENT_CHAR_SCALE).max(1.0);
        let padding = (cell_width * 2.0).max(12.0);
        let line_height = (cell_height * 1.5).max(1.0);
        let title_height = cell_height * 2.5;
        let footer_height = cell_height * 4.5;
        let outer_margin = (cell_width * 2.0).max(12.0);
        let scrollbar_gutter = cell_width.max(6.0);

        let preferred_width = (content_columns.max(title_columns) as f32 * char_scale)
            + padding * 2.0
            + scrollbar_gutter;
        let available_width = (window_width - outer_margin * 2.0).max(1.0);
        let card_width = preferred_width.max(min_width).min(available_width);

        let fixed_height = padding + title_height + footer_height + padding * 0.5;
        let available_height = (window_height - outer_margin * 2.0).max(line_height);
        let content_budget = (available_height - fixed_height).max(line_height);
        let rows_that_fit = ((content_budget / line_height).floor() as usize).max(1);
        let visible_rows = total_rows.max(1).min(rows_that_fit);
        let content_height = visible_rows as f32 * line_height;
        let card_height = fixed_height + content_height;
        let card_x = (window_width - card_width) / 2.0;
        let card_y = (window_height - card_height) / 2.0 + (1.0 - animation) * -20.0;

        let content_x = card_x + padding;
        let content_y = card_y + padding + title_height;
        let content_width = (card_width - padding * 2.0 - scrollbar_gutter).max(char_scale);
        let columns_that_fit = ((content_width / char_scale).floor() as usize).max(1);

        Self {
            card: Bounds::new(card_x, card_y, card_width, card_height),
            content: Bounds::new(content_x, content_y, content_width, content_height),
            padding,
            title_height,
            line_height,
            char_scale,
            footer_height,
            rows_that_fit,
            columns_that_fit,
            button_y: content_y + content_height + cell_height * 1.3,
            button_height: cell_height * 1.8,
            button_gap: cell_width * 1.5,
        }
    }

    /// A button on the button row starting at `x`.
    pub fn button(&self, x: f32, width: f32) -> Bounds {
        Bounds::new(x, self.button_y, width.max(1.0), self.button_height)
    }

    /// Width of the button row inside the card's padding, less `gaps`
    /// spaces between buttons.
    pub fn button_row_width(&self, gaps: usize) -> f32 {
        self.card.width - self.padding * 2.0 - self.button_gap * gaps as f32
    }
}

/// One scroll of a card's text: by rows or columns, or to an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayScroll {
    Rows(isize),
    Columns(isize),
    Top,
    Bottom,
    LineStart,
    LineEnd,
}

/// The part of a card's text in view: `rows` × `columns` cells scrolled
/// over `content_rows` lines at most `content_columns` wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextViewport {
    row: usize,
    column: usize,
    rows: usize,
    columns: usize,
    content_rows: usize,
    content_columns: usize,
}

impl Default for TextViewport {
    fn default() -> Self {
        Self {
            row: 0,
            column: 0,
            rows: 1,
            columns: 1,
            content_rows: 0,
            content_columns: 0,
        }
    }
}

impl TextViewport {
    /// Show new content from its top-left corner.
    pub fn reset(&mut self, content_rows: usize, content_columns: usize) {
        self.row = 0;
        self.column = 0;
        self.set_content(content_rows, content_columns);
    }

    /// The content changed size in place; keep the scroll where it can stay.
    pub fn set_content(&mut self, content_rows: usize, content_columns: usize) {
        self.content_rows = content_rows;
        self.content_columns = content_columns;
        self.clamp();
    }

    /// The card now shows `rows` × `columns` cells.
    pub fn resize(&mut self, rows: usize, columns: usize) {
        self.rows = rows.max(1);
        self.columns = columns.max(1);
        self.clamp();
    }

    /// Scroll by whole rows and columns; whether anything moved.
    pub fn scroll_by(&mut self, rows: isize, columns: isize) -> bool {
        let before = (self.row, self.column);
        self.row = self.row.saturating_add_signed(rows);
        self.column = self.column.saturating_add_signed(columns);
        self.clamp();
        before != (self.row, self.column)
    }

    /// Scroll to a first row and/or column; whether anything moved.
    pub fn scroll_to(&mut self, row: Option<usize>, column: Option<usize>) -> bool {
        let before = (self.row, self.column);
        self.row = row.unwrap_or(self.row);
        self.column = column.unwrap_or(self.column);
        self.clamp();
        before != (self.row, self.column)
    }

    /// Apply one scroll; whether anything moved.
    pub fn apply(&mut self, scroll: OverlayScroll) -> bool {
        match scroll {
            OverlayScroll::Rows(delta) => self.scroll_by(delta, 0),
            OverlayScroll::Columns(delta) => self.scroll_by(0, delta),
            OverlayScroll::Top => self.scroll_to(Some(0), None),
            OverlayScroll::Bottom => self.scroll_to(Some(self.max_row()), None),
            OverlayScroll::LineStart => self.scroll_to(None, Some(0)),
            OverlayScroll::LineEnd => self.scroll_to(None, Some(self.max_column())),
        }
    }

    /// Scroll as little as possible to show `row`.
    pub fn reveal_row(&mut self, row: usize) {
        self.row = reveal_row(self.row, row, self.rows);
        self.clamp();
    }

    /// Scroll as little as possible to show `column`.
    pub fn reveal_column(&mut self, column: usize) {
        self.column = reveal_row(self.column, column, self.columns);
        self.clamp();
    }

    /// The first row in view.
    pub fn row(&self) -> usize {
        self.row
    }

    /// The first column in view.
    pub fn column(&self) -> usize {
        self.column
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn max_row(&self) -> usize {
        self.content_rows.saturating_sub(self.rows)
    }

    pub fn max_column(&self) -> usize {
        self.content_columns.saturating_sub(self.columns)
    }

    /// The content columns the widest line spans.
    pub fn content_columns(&self) -> usize {
        self.content_columns
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.max_row());
        self.column = self.column.min(self.max_column());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewport_scrolls_both_axes_within_its_content() {
        let mut viewport = TextViewport::default();
        viewport.reset(100, 40);
        viewport.resize(10, 30);
        assert!(viewport.scroll_by(5, 4));
        assert_eq!((viewport.row(), viewport.column()), (5, 4));
        assert!(viewport.scroll_by(1_000, 1_000));
        assert_eq!((viewport.row(), viewport.column()), (90, 10));
        assert!(!viewport.scroll_by(1, 1), "already at the far corner");
        assert!(viewport.scroll_to(Some(0), None));
        assert_eq!((viewport.row(), viewport.column()), (0, 10));
        viewport.reveal_row(25);
        viewport.reveal_column(2);
        assert_eq!((viewport.row(), viewport.column()), (16, 2));
        viewport.set_content(12, 8);
        assert_eq!((viewport.row(), viewport.column()), (2, 0));
        viewport.reset(3, 3);
        assert_eq!((viewport.max_row(), viewport.max_column()), (0, 0));
    }

    #[test]
    fn each_scroll_moves_its_axis_to_the_step_or_edge() {
        let mut viewport = TextViewport::default();
        viewport.reset(100, 40);
        viewport.resize(10, 30);
        let at = |viewport: &TextViewport| (viewport.row(), viewport.column());
        assert!(viewport.apply(OverlayScroll::Rows(3)));
        assert!(viewport.apply(OverlayScroll::Columns(2)));
        assert_eq!(at(&viewport), (3, 2));
        assert!(viewport.apply(OverlayScroll::Bottom));
        assert!(viewport.apply(OverlayScroll::LineEnd));
        assert_eq!(at(&viewport), (90, 10));
        assert!(
            !viewport.apply(OverlayScroll::Bottom),
            "already at the bottom"
        );
        assert!(viewport.apply(OverlayScroll::Top));
        assert!(viewport.apply(OverlayScroll::LineStart));
        assert_eq!(at(&viewport), (0, 0));
    }

    #[test]
    fn cards_stay_in_small_windows_and_slide_with_their_animation() {
        let open = TextCard::calculate(300.0, 200.0, 10.0, 20.0, 1.0, 400, 20, 380.0, 1_000);
        assert!(open.card.x >= 0.0 && open.card.right() <= 300.0);
        assert!(open.card.y >= 0.0 && open.card.bottom() <= 200.0);
        assert!(open.content.right() < open.card.right() - open.padding);
        assert_eq!(open.rows_that_fit, 1);
        let closed =
            TextCard::calculate(300.0, 200.0, 10.0, 20.0, 1.0 - 0.5, 400, 20, 380.0, 1_000);
        assert_eq!(closed.card.y, open.card.y - 10.0);
        assert_eq!(closed.button_y, open.button_y - 10.0);
    }
}
