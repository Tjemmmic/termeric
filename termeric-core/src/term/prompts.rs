//! Shell integration: prompt marks and prompt navigation.

use super::*;

impl Terminal {
    /// Handle OSC 133 shell integration sequence
    pub(crate) fn handle_osc_133(&mut self, params: &[&[u8]]) {
        if self.alt_screen_active {
            log::trace!("Ignoring OSC 133 from the alternate screen");
            return;
        }
        let cursor = self.grid().cursor();
        let cursor_row = cursor.row;
        // A cursor waiting to wrap sits past the last cell, not on it.
        let cursor_col = cursor.col + usize::from(cursor.wrap_pending);
        if params.first().is_some_and(|value| *value == b"A") {
            self.prompt_navigation = None;
        }
        // The command runs from where the prompt ended to where output
        // begins; read its start before output-start closes it.
        let command_start = self.prompt_tracker.pending_command_start();
        match self.prompt_tracker.handle_osc_133(params, cursor_row) {
            Some(PromptType::CommandStart) => {
                self.prompt_tracker.set_command_start_column(cursor_col);
            }
            Some(PromptType::OutputStart) => {
                let (text, truncated) = match command_start {
                    Some(start) => self.command_text_between(
                        start,
                        (i32::try_from(cursor_row).unwrap_or(i32::MAX), cursor_col),
                        MAX_COMMAND_TEXT_BYTES,
                    ),
                    // The start was erased or lost in a reflow.
                    None => (String::new(), true),
                };
                self.prompt_tracker.record_command_text(text, truncated);
            }
            _ => {}
        }
    }

    /// Navigate to the previous retained OSC 133 prompt from the current
    /// prompt-navigation cursor or the top of a manually scrolled viewport.
    pub fn previous_prompt(&mut self) -> Option<i32> {
        if self.alt_screen_active {
            self.prompt_navigation = None;
            return None;
        }
        let from_row = self.prompt_navigation_origin(true);
        let row = self.prompt_tracker.previous_prompt(from_row)?;
        self.reveal_prompt(row);
        Some(row)
    }

    /// Navigate to the next retained OSC 133 prompt from the current
    /// prompt-navigation cursor or the bottom of a manually scrolled viewport.
    pub fn next_prompt(&mut self) -> Option<i32> {
        if self.alt_screen_active {
            self.prompt_navigation = None;
            return None;
        }
        let from_row = self.prompt_navigation_origin(false);
        let row = self.prompt_tracker.next_prompt(from_row)?;
        self.reveal_prompt(row);
        Some(row)
    }

    /// Navigate to the prompt of the nearest earlier command that exited
    /// non-zero, using the same origin as prompt navigation.
    pub fn previous_failed_command(&mut self) -> Option<i32> {
        if self.alt_screen_active {
            self.prompt_navigation = None;
            return None;
        }
        let from_row = self.prompt_navigation_origin(true);
        let row = self.prompt_tracker.previous_failed_prompt(from_row)?;
        self.reveal_prompt(row);
        Some(row)
    }

    /// Navigate to the prompt of the nearest later command that exited
    /// non-zero.
    pub fn next_failed_command(&mut self) -> Option<i32> {
        if self.alt_screen_active {
            self.prompt_navigation = None;
            return None;
        }
        let from_row = self.prompt_navigation_origin(false);
        let row = self.prompt_tracker.next_failed_prompt(from_row)?;
        self.reveal_prompt(row);
        Some(row)
    }

    fn prompt_navigation_origin(&mut self, previous: bool) -> i32 {
        let offset = self.grid().scroll_offset();
        if let Some(navigation) = self.prompt_navigation
            && navigation.scroll_offset == offset
            && self
                .prompt_tracker
                .all_prompts()
                .any(|row| row == navigation.row)
        {
            return navigation.row;
        }
        self.prompt_navigation = None;

        if offset == 0 {
            return i32::try_from(self.grid().cursor().row).unwrap_or(i32::MAX);
        }
        let offset = i32::try_from(offset).unwrap_or(i32::MAX);
        if previous {
            offset.saturating_neg()
        } else {
            i32::try_from(self.rows)
                .unwrap_or(i32::MAX)
                .saturating_sub(1)
                .saturating_sub(offset)
        }
    }

    pub(super) fn reveal_prompt(&mut self, row: i32) {
        let offset = if row < 0 {
            usize::try_from(row.saturating_neg()).unwrap_or(usize::MAX)
        } else {
            0
        };
        self.grid_mut().set_scroll_offset(offset);
        self.prompt_navigation = Some(PromptNavigationState {
            row,
            scroll_offset: self.grid().scroll_offset(),
        });
    }

    pub(super) fn shift_prompts_after_archival_scroll(&mut self, bottom: usize) {
        if self.alt_screen_active {
            return;
        }
        let oldest = -(self.grid().scrollback_len() as i32);
        let cursor_row = self.grid().cursor().row;
        self.prompt_tracker
            .adjust_for_archival_region(bottom, oldest, cursor_row);
        if let Some(navigation) = &mut self.prompt_navigation
            && navigation.row < i32::try_from(bottom).unwrap_or(i32::MAX)
        {
            navigation.row = navigation.row.saturating_sub(1);
        }
        self.retain_prompt_navigation(oldest);
    }

    pub(super) fn retain_prompt_navigation(&mut self, oldest_retained_row: i32) {
        let retained = self.prompt_navigation.is_some_and(|navigation| {
            navigation.row >= oldest_retained_row
                && self
                    .prompt_tracker
                    .all_prompts()
                    .any(|row| row == navigation.row)
        });
        if retained {
            let row = self.prompt_navigation.unwrap().row;
            if row < 0 && self.grid.scroll_offset() == 0 {
                let offset = usize::try_from(row.saturating_neg()).unwrap_or(usize::MAX);
                self.grid.set_scroll_offset(offset);
            }
            if let Some(navigation) = &mut self.prompt_navigation {
                navigation.scroll_offset = self.grid.scroll_offset();
            }
        } else {
            self.prompt_navigation = None;
        }
    }

    pub(super) fn invalidate_prompt_navigation_after_manual_scroll(&mut self) {
        if self
            .prompt_navigation
            .is_some_and(|navigation| navigation.scroll_offset != self.grid.scroll_offset())
        {
            self.prompt_navigation = None;
        }
    }

    /// OSC 133 prompt and command regions reported by the shell.
    pub fn prompt_tracker(&self) -> &PromptTracker {
        &self.prompt_tracker
    }
}
