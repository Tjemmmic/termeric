//! Native selection follows the displayed grid; PTY mouse reports remain live.

use super::*;
use termeric_core::Point;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct MouseSelectionPresentation {
    owner: crate::tabs::TerminalIdentity,
    deadline: Option<Instant>,
    alternate_screen: bool,
    cols: usize,
    rows: usize,
    filter_generation: Option<u64>,
}

impl App {
    fn mouse_selection_presentation(&self) -> Option<MouseSelectionPresentation> {
        let owner = self.focused_pointer_terminal()?;
        let terminal = self.active_terminal()?;
        let grid = terminal.presentation_grid();
        Some(MouseSelectionPresentation {
            owner,
            deadline: terminal.synchronized_output_deadline(),
            alternate_screen: terminal.presentation_alternate_screen_active(),
            cols: grid.cols(),
            rows: grid.rows(),
            filter_generation: self.search.filter_generation(),
        })
    }

    pub(super) fn reconcile_mouse_selection_presentation(&mut self) -> bool {
        self.reconcile_search_presentation();
        if self.mouse_selection_source.is_none()
            || self.mouse_selection_source == self.mouse_selection_presentation()
        {
            return false;
        }
        let previous = self.mouse_selection_source.unwrap();
        if (previous.filter_generation.is_some() || self.search.filter_generation().is_some())
            && let Some(tabs) = &mut self.tabs
            && let Some(origin) =
                tabs.origin_for_pane(previous.owner.tab_id, previous.owner.pane_id)
            && let Some(terminal) = tabs.terminal_for_origin_mut(origin)
        {
            // A projected range must never survive as an ordinary contiguous
            // selection when its filter or focused source disappears.
            terminal.set_selection(None);
        }
        self.mouse_pressed = false;
        self.mouse_state.clear_selection();
        self.mouse_state.click_count = 0;
        self.mouse_state.last_click_pos = None;
        self.mouse_selection_source = None;
        true
    }

    /// Existing pointer conversion returns live scrollback coordinates. Undo
    /// that offset, clamp to the viewport, then apply the displayed offset.
    pub(super) fn native_selection_point(&self, live_point: Point) -> Option<Point> {
        self.projected_native_selection_point(live_point, false)
    }

    fn projected_native_selection_point(&self, live_point: Point, dragging: bool) -> Option<Point> {
        let terminal = self.active_terminal()?;
        let live_offset = i32::try_from(terminal.grid().scroll_offset()).ok()?;
        let grid = terminal.presentation_grid();
        let displayed_offset = i32::try_from(grid.scroll_offset()).ok()?;
        let visual_row = live_point
            .row
            .checked_add(live_offset)?
            .clamp(0, grid.rows().saturating_sub(1) as i32);
        let filtered = self.search.filtered_rows(grid.rows());
        let row = if filtered.is_empty() {
            visual_row.saturating_sub(displayed_offset)
        } else {
            let index = if dragging {
                (visual_row as usize).min(filtered.len() - 1)
            } else {
                visual_row as usize
            };
            *filtered.get(index)?
        };
        Some(Point::new(
            row,
            live_point.col.min(grid.cols().saturating_sub(1)),
        ))
    }

    pub(super) fn begin_native_selection(&mut self, button: MouseButton, live_point: Point) {
        self.reconcile_mouse_selection_presentation();
        let Some(source) = self.mouse_selection_presentation() else {
            return;
        };
        let Some(point) = self.native_selection_point(live_point) else {
            return;
        };
        let Some(mut selection) = self.mouse_state.handle_press(button, point, self.modifiers)
        else {
            return;
        };
        self.mouse_selection_source = Some(source);
        self.mouse_pressed = true;
        if let Some(terminal) = self.active_terminal_mut() {
            let bounds = match selection.ty {
                termeric_core::SelectionType::Semantic => Some(terminal.word_at(point)),
                termeric_core::SelectionType::Lines => Some(terminal.line_at(point)),
                _ => None,
            };
            if let Some((start, end)) = bounds {
                selection.start = start;
                selection.end = end;
            }
            terminal.set_selection(Some(selection.clone()));
        }
        self.mouse_state.selection = Some(selection);
    }

    pub(super) fn extend_native_selection(&mut self, live_point: Point) {
        if self.reconcile_mouse_selection_presentation() || !self.mouse_pressed {
            return;
        }
        let Some(point) = self.projected_native_selection_point(live_point, true) else {
            return;
        };
        if let Some(selection) = self.mouse_state.handle_move(point)
            && let Some(terminal) = self.active_terminal_mut()
        {
            terminal.set_selection(Some(selection));
        }
    }

    pub(super) fn finish_native_selection(&mut self) {
        let was_pressed = self.mouse_pressed;
        self.mouse_pressed = false;
        let _ = self.mouse_state.handle_release(MouseButton::Left);
        if was_pressed
            && self.mouse_selection_source.is_some()
            && self.mouse_selection_source == self.mouse_selection_presentation()
        {
            let selection = self.mouse_state.selection.clone();
            if let Some(terminal) = self.active_terminal_mut() {
                terminal.set_selection(selection);
            }
        }
    }

    pub(super) fn native_selection_text(&self) -> Option<String> {
        if let Some(source) = self.mouse_selection_source
            && (source.filter_generation.is_some() || self.search.filter_generation().is_some())
            && Some(source) != self.mouse_selection_presentation()
        {
            return None;
        }
        let terminal = self.active_terminal()?;
        let rows = self.search.filtered_matching_rows();
        if rows.is_empty() {
            terminal.get_selection_text()
        } else {
            Some(terminal.presentation_selection_text_in_rows(terminal.selection()?, rows))
        }
    }
}
