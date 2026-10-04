//! The text of one grid row as search, URL detection, and quick select read
//! it, with each cell's place in that text.

use std::ops::Range;
use termeric_core::{CellContent, RowView};

/// Appends the text of `row`'s cells to `text`: the cells a reflow keeps
/// (wide-wrap padding is not text), with an empty cell read as a space and a
/// multi-cell glyph read once. For each cell, `on_cell` receives its column,
/// display width, and byte range in `text`.
pub(crate) fn push_row_text(
    row: &RowView<'_>,
    text: &mut String,
    mut on_cell: impl FnMut(usize, usize, Range<usize>),
) {
    for col in row.reflow_columns() {
        let Some(cell) = row.get(col) else { continue };
        if cell.multicell.is_present() && !cell.multicell.is_leader() {
            continue;
        }
        let start = text.len();
        match &cell.content {
            CellContent::Empty => text.push(' '),
            CellContent::Char(character) => text.push(*character),
            CellContent::Grapheme(grapheme) => text.push_str(grapheme),
        }
        on_cell(col, cell.width(), start..text.len());
    }
}
