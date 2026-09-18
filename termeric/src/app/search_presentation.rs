//! Bind search results and row coordinates to the displayed terminal grid.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchPresentation {
    tab: usize,
    pane: Option<PaneId>,
    deadline: Option<Instant>,
    alternate_screen: bool,
    cols: usize,
    rows: usize,
}

impl App {
    /// Retain the query and controls but discard results/partial work when its
    /// displayed source changes. Return true only when a scoped search closes.
    pub(super) fn reconcile_search_presentation(&mut self) -> bool {
        let next = self.tabs.as_ref().and_then(|tabs| {
            let tab = tabs.active_tab()?;
            let terminal = tab.focused_terminal()?;
            let grid = terminal.presentation_grid();
            Some(SearchPresentation {
                tab: tab.id,
                pane: tab
                    .pane_manager
                    .as_ref()
                    .map(crate::panes::PaneManager::focused_id),
                deadline: terminal.synchronized_output_deadline(),
                alternate_screen: terminal.presentation_alternate_screen_active(),
                cols: grid.cols(),
                rows: grid.rows(),
            })
        });
        if self.search_presentation == next {
            return false;
        }
        let previous = std::mem::replace(&mut self.search_presentation, next);
        if self.search.is_scoped() && previous.is_some() {
            // Command-region coordinates cannot be transferred across panes,
            // reflow or a viewport-only snapshot. Never widen the user's scope.
            self.search.cancel();
            self.search_cursor_pos = 0;
            self.toasts
                .info("Command-output search closed because its displayed source changed");
            return true;
        }
        if let Some(terminal) = self
            .tabs
            .as_ref()
            .and_then(TabManager::active_tab)
            .and_then(crate::tabs::Tab::focused_terminal)
        {
            self.search.refresh(terminal.presentation_grid());
        } else {
            self.search.cancel();
            self.search_cursor_pos = 0;
        }
        false
    }

    pub(super) fn start_command_output_search(&mut self, first: i32, last: i32) -> bool {
        self.reconcile_search_presentation();
        if self
            .active_terminal()
            .is_none_or(|terminal| terminal.synchronized_output_deadline().is_some())
        {
            self.toasts.warning(
                "Wait for synchronized output to finish before searching a command's output",
            );
            return false;
        }
        self.search.start_in_rows(first, last);
        self.search_cursor_pos = 0;
        true
    }
}
