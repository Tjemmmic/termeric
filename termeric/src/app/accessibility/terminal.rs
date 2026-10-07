//! Terminal panes as text: each visible row a text run, with the selection
//! and caret placed in characters.

use super::*;

pub(super) fn append_terminal_nodes(
    nodes: &mut Vec<(NodeId, Node)>,
    terminal_id: NodeId,
    pane_id: PaneId,
    terminal: &termeric_core::Terminal,
    focused: bool,
    broadcast_target: Option<crate::renderer::BroadcastTargetState>,
    read_only: bool,
    zoomed: bool,
    bounds: Rect,
    cell_height: f32,
    filtered_rows: &[i32],
) {
    let grid = terminal.presentation_grid();
    let mut line_ids = Vec::with_capacity(grid.rows());
    let mut lines = Vec::with_capacity(grid.rows());
    let row_count = if filtered_rows.is_empty() {
        grid.rows()
    } else {
        filtered_rows.len()
    };
    for row in 0..row_count {
        let line_id = line_node(pane_id, row);
        let line = if filtered_rows.is_empty() {
            accessible_line(grid, row)
        } else {
            accessible_source_line(grid, filtered_rows[row])
        };
        let mut node = Node::new(Role::TextRun);
        node.set_value(line.value.clone());
        node.set_character_lengths(line.character_lengths.clone());
        node.set_bounds(rect(
            bounds.x0 as f32,
            bounds.y0 as f32 + row as f32 * cell_height,
            bounds.width() as f32,
            cell_height,
        ));
        nodes.push((line_id, node));
        line_ids.push(line_id);
        lines.push(line);
    }

    let mut node = Node::new(Role::Terminal);
    let mut label = format!("Terminal pane {}", pane_id.0 + 1);
    if zoomed {
        label.push_str(", zoomed");
    }
    if read_only {
        label.push_str(", read-only input lock active");
    }
    match broadcast_target {
        Some(crate::renderer::BroadcastTargetState::Armed) => {
            label.push_str(", selected broadcast target, not active")
        }
        Some(crate::renderer::BroadcastTargetState::Active) => {
            label.push_str(", active broadcast target")
        }
        None => {}
    }
    node.set_label(label);
    node.set_description("Visible terminal text; off-screen rows are not exposed");
    node.set_bounds(bounds);
    node.set_children(line_ids);
    node.set_live(Live::Polite);
    node.set_read_only();
    node.add_action(Action::Focus);
    node.add_action(Action::Click);
    if focused && !lines.is_empty() {
        let selection = terminal
            .selection()
            .and_then(|selection| {
                if filtered_rows.is_empty() {
                    accessible_selection(selection, pane_id, &lines, grid.scroll_offset())
                } else {
                    accessible_filtered_selection(selection, pane_id, &lines, filtered_rows)
                }
            })
            .or_else(|| {
                if grid.scroll_offset() != 0 || !filtered_rows.is_empty() {
                    return None;
                }
                let cursor = grid.cursor();
                let row = cursor.row.min(lines.len() - 1);
                let index = lines[row].character_index(cursor.col);
                let position = TextPosition {
                    node: line_node(pane_id, row),
                    character_index: index,
                };
                Some(TextSelection {
                    anchor: position,
                    focus: position,
                })
            });
        if let Some(selection) = selection {
            node.set_text_selection(selection);
        }
    }
    nodes.push((terminal_id, node));
}

#[derive(Debug, PartialEq, Eq)]
struct AccessibleLine {
    value: String,
    character_lengths: Vec<u8>,
    column_to_character: Vec<usize>,
}

impl AccessibleLine {
    pub(super) fn character_index(&self, column: usize) -> usize {
        self.column_to_character
            .get(column)
            .copied()
            .unwrap_or(self.character_lengths.len())
            .min(self.character_lengths.len())
    }
}

fn accessible_line(grid: &Grid, row: usize) -> AccessibleLine {
    let source =
        (row as i32).saturating_sub(i32::try_from(grid.scroll_offset()).unwrap_or(i32::MAX));
    accessible_source_line(grid, source)
}

fn accessible_source_line(grid: &Grid, row: i32) -> AccessibleLine {
    let mut value = String::new();
    let mut character_lengths = Vec::new();
    let mut column_to_character = Vec::with_capacity(grid.cols() + 1);
    let source = grid.row_at_offset(row);
    for col in 0..grid.cols() {
        column_to_character.push(character_lengths.len());
        if let Some(cell) = source.as_ref().and_then(|row| row.get(col)) {
            append_accessible_cell(&cell, &mut value, &mut character_lengths);
        } else {
            value.push(' ');
            character_lengths.push(1);
        }
    }
    column_to_character.push(character_lengths.len());
    while value.ends_with(' ') {
        value.pop();
        character_lengths.pop();
    }
    AccessibleLine {
        value,
        character_lengths,
        column_to_character,
    }
}

pub(super) fn append_accessible_cell(cell: &Cell, value: &mut String, lengths: &mut Vec<u8>) {
    if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER)
        || (cell.multicell.is_present() && !cell.multicell.is_leader())
    {
        return;
    }
    if cell.attrs.contains(CellAttrs::HIDDEN) || cell.content.is_empty() {
        value.push(' ');
        lengths.push(1);
        return;
    }
    let mut content = String::new();
    cell.content.push_to(&mut content);
    for character in content.chars() {
        value.push(character);
        lengths.push(character.len_utf8() as u8);
    }
}

fn accessible_filtered_selection(
    selection: &Selection,
    pane_id: PaneId,
    lines: &[AccessibleLine],
    rows: &[i32],
) -> Option<TextSelection> {
    let (start, end) = selection.bounds();
    let first = rows.partition_point(|row| *row < start.row);
    let after_last = rows.partition_point(|row| *row <= end.row);
    if first >= after_last {
        return None;
    }
    let last = after_last - 1;
    let start_col = if rows[first] == start.row {
        start.col
    } else {
        0
    };
    let end_col = if rows[last] == end.row {
        end.col
    } else {
        lines.get(last)?.column_to_character.len().saturating_sub(2)
    };
    let start_point = termeric_core::Point::new(first as i32, start_col);
    let end_point = termeric_core::Point::new(last as i32, end_col);
    let mut projected = selection.clone();
    let forward = selection.start <= selection.end;
    projected.start = if forward { start_point } else { end_point };
    projected.end = if forward { end_point } else { start_point };
    accessible_selection(&projected, pane_id, lines, 0)
}

fn accessible_selection(
    selection: &Selection,
    pane_id: PaneId,
    lines: &[AccessibleLine],
    scroll_offset: usize,
) -> Option<TextSelection> {
    let last_row = i64::try_from(lines.len().checked_sub(1)?).ok()?;
    let offset = i64::try_from(scroll_offset).unwrap_or(i64::MAX);
    let (first, last) = selection.bounds();
    let first_visible = i64::from(first.row).saturating_add(offset);
    let last_visible = i64::from(last.row).saturating_add(offset);
    if last_visible < 0 || first_visible > last_row {
        return None;
    }
    let position = |point: termeric_core::Point, after_cell: bool| {
        let row = i64::from(point.row)
            .saturating_add(offset)
            .clamp(0, last_row) as usize;
        Some(TextPosition {
            node: line_node(pane_id, row),
            character_index: lines[row]
                .character_index(point.col.saturating_add(usize::from(after_cell))),
        })
    };
    let forward = selection.start <= selection.end;
    Some(TextSelection {
        anchor: position(selection.start, !forward)?,
        focus: position(selection.end, forward)?,
    })
}
