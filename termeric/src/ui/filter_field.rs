//! Filter fields: the query fields with a leading magnifier (Settings,
//! Shortcuts, and the command palette) that the renderer draws with
//! `draw_filter_field`.

use crate::ui::constants::{FILTER_TEXT_END_INSET_CELLS, FILTER_TEXT_INSET_CELLS};
use crate::ui::geometry::Bounds;
use crate::ui::text_input_viewport::TextInputViewport;

/// The visible part of a filter field's text: after the magnifier, centred
/// vertically, and short of the field's right edge. Painting, caret clicks,
/// and the IME caret all use it.
pub fn text_viewport(
    text: &str,
    cursor: usize,
    field: Bounds,
    cell_width: f32,
    cell_height: f32,
) -> Option<TextInputViewport> {
    TextInputViewport::calculate(
        text,
        cursor,
        field.x + cell_width * FILTER_TEXT_INSET_CELLS,
        field.y + (field.height - cell_height) / 2.0,
        field.right() - cell_width * FILTER_TEXT_END_INSET_CELLS,
        cell_width,
    )
}
