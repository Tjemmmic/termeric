//! Prompt tracking for shell integration
//!
//! Tracks prompt positions and command regions using OSC 133 sequences.

use std::collections::VecDeque;

/// Maximum number of prompts to track
const MAX_PROMPTS: usize = 1000;

/// Type of prompt mark
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptType {
    /// Fresh prompt start (OSC 133;A)
    PromptStart,
    /// Command input start (OSC 133;B)
    CommandStart,
    /// Output start (OSC 133;C)
    OutputStart,
    /// Command finished (OSC 133;D)
    CommandFinished,
}

/// A mark in the terminal indicating a prompt or command boundary
#[derive(Debug, Clone)]
pub struct PromptMark {
    /// Row where this mark occurred
    pub row: usize,
    /// Type of mark
    pub mark_type: PromptType,
    /// Exit code for CommandFinished marks
    pub exit_code: Option<i32>,
}

/// A complete command region with prompt, command, and output
#[derive(Debug, Clone)]
pub struct CommandRegion {
    /// Row where the prompt started
    pub prompt_row: usize,
    /// Row where command input started
    pub command_row: Option<usize>,
    /// Row where output started
    pub output_row: Option<usize>,
    /// Row where command finished
    pub finish_row: Option<usize>,
    /// Exit code
    pub exit_code: Option<i32>,
}

impl CommandRegion {
    /// Check if the command succeeded (exit code 0)
    pub fn is_success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// Check if the command failed (non-zero exit code)
    pub fn is_failure(&self) -> bool {
        matches!(self.exit_code, Some(code) if code != 0)
    }

    /// Check if the command is still running
    pub fn is_running(&self) -> bool {
        self.command_row.is_some() && self.finish_row.is_none()
    }
}

/// Tracks prompts and command regions in the terminal
#[derive(Debug, Default)]
pub struct PromptTracker {
    /// All recorded marks
    marks: VecDeque<PromptMark>,
    /// Completed command regions
    regions: VecDeque<CommandRegion>,
    /// Current incomplete region being built
    current_region: Option<CommandRegion>,
}

impl PromptTracker {
    /// Create a new prompt tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the start of a prompt (OSC 133;A)
    pub fn mark_prompt_start(&mut self, row: usize) {
        // Finalize any previous incomplete region
        self.finalize_current_region();

        self.add_mark(PromptMark {
            row,
            mark_type: PromptType::PromptStart,
            exit_code: None,
        });

        self.current_region = Some(CommandRegion {
            prompt_row: row,
            command_row: None,
            output_row: None,
            finish_row: None,
            exit_code: None,
        });
    }

    /// Mark the start of command input (OSC 133;B)
    pub fn mark_command_start(&mut self, row: usize) {
        self.add_mark(PromptMark {
            row,
            mark_type: PromptType::CommandStart,
            exit_code: None,
        });

        if let Some(ref mut region) = self.current_region {
            region.command_row = Some(row);
        }
    }

    /// Mark the start of command output (OSC 133;C)
    pub fn mark_output_start(&mut self, row: usize) {
        self.add_mark(PromptMark {
            row,
            mark_type: PromptType::OutputStart,
            exit_code: None,
        });

        if let Some(ref mut region) = self.current_region {
            region.output_row = Some(row);
        }
    }

    /// Mark the command as finished (OSC 133;D)
    pub fn mark_command_finished(&mut self, row: usize, exit_code: Option<i32>) {
        self.add_mark(PromptMark {
            row,
            mark_type: PromptType::CommandFinished,
            exit_code,
        });

        if let Some(ref mut region) = self.current_region {
            region.finish_row = Some(row);
            region.exit_code = exit_code;
        }

        self.finalize_current_region();
    }

    /// Add a mark and enforce capacity limits. Logs a warning the first
    /// time the cap is exceeded so users know navigation history is
    /// capped in long-running shell sessions.
    fn add_mark(&mut self, mark: PromptMark) {
        if self.marks.len() >= MAX_PROMPTS {
            if self.marks.len() == MAX_PROMPTS {
                log::warn!(
                    "Prompt history reached {} marks; oldest prompts will now be dropped",
                    MAX_PROMPTS
                );
            }
            self.marks.pop_front();
        }
        self.marks.push_back(mark);
    }

    /// Finalize the current region and add it to the regions list
    fn finalize_current_region(&mut self) {
        if let Some(region) = self.current_region.take() {
            if self.regions.len() >= MAX_PROMPTS {
                if self.regions.len() == MAX_PROMPTS {
                    log::warn!(
                        "Command history reached {} entries; oldest will now be dropped",
                        MAX_PROMPTS
                    );
                }
                self.regions.pop_front();
            }
            self.regions.push_back(region);
        }
    }

    /// Get the previous prompt row from the given position
    pub fn previous_prompt(&self, from_row: i32) -> Option<usize> {
        self.marks
            .iter()
            .rev()
            .find(|m| m.mark_type == PromptType::PromptStart && (m.row as i32) < from_row)
            .map(|m| m.row)
    }

    /// Get the next prompt row from the given position
    pub fn next_prompt(&self, from_row: i32) -> Option<usize> {
        self.marks
            .iter()
            .find(|m| m.mark_type == PromptType::PromptStart && (m.row as i32) > from_row)
            .map(|m| m.row)
    }

    /// Get all prompt rows
    pub fn all_prompts(&self) -> impl Iterator<Item = usize> + '_ {
        self.marks
            .iter()
            .filter(|m| m.mark_type == PromptType::PromptStart)
            .map(|m| m.row)
    }

    /// Get the command region containing a given row
    pub fn command_at(&self, row: i32) -> Option<&CommandRegion> {
        let row = row as usize;

        // Check current region first
        if let Some(ref region) = self.current_region {
            let end = region.finish_row.unwrap_or(usize::MAX);
            if row >= region.prompt_row && row <= end {
                return self.current_region.as_ref();
            }
        }

        // Search completed regions
        self.regions.iter().rev().find(|region| {
            let end = region.finish_row.unwrap_or(usize::MAX);
            row >= region.prompt_row && row <= end
        })
    }

    /// Get the number of completed command regions
    /// The command region currently in progress (if any). Used by callers
    /// that want to react to the active command without waiting for it to
    /// finish (e.g. live growth indicators).
    pub fn active_region(&self) -> Option<&CommandRegion> {
        self.current_region.as_ref()
    }

    pub fn completed_count(&self) -> usize {
        self.regions.len()
    }

    /// Get a completed region by index
    pub fn completed_region(&self, index: usize) -> Option<&CommandRegion> {
        self.regions.get(index)
    }

    /// Clear all tracking data
    pub fn clear(&mut self) {
        self.marks.clear();
        self.regions.clear();
        self.current_region = None;
    }

    /// Adjust row numbers after scrolling
    ///
    /// Call this when the terminal scrolls to update row references.
    /// Lines that scroll off screen (row < 0 after adjustment) are removed.
    pub fn adjust_for_scroll(&mut self, scroll_amount: i32) {
        // Adjust marks
        self.marks.retain_mut(|mark| {
            let new_row = mark.row as i32 - scroll_amount;
            if new_row >= 0 {
                mark.row = new_row as usize;
                true
            } else {
                false
            }
        });

        // Adjust regions
        self.regions.retain_mut(|region| {
            let new_prompt_row = region.prompt_row as i32 - scroll_amount;
            if new_prompt_row >= 0 {
                region.prompt_row = new_prompt_row as usize;
                if let Some(ref mut row) = region.command_row {
                    *row = (*row as i32 - scroll_amount).max(0) as usize;
                }
                if let Some(ref mut row) = region.output_row {
                    *row = (*row as i32 - scroll_amount).max(0) as usize;
                }
                if let Some(ref mut row) = region.finish_row {
                    *row = (*row as i32 - scroll_amount).max(0) as usize;
                }
                true
            } else {
                false
            }
        });

        // Adjust current region
        if let Some(ref mut region) = self.current_region {
            let new_prompt_row = region.prompt_row as i32 - scroll_amount;
            if new_prompt_row >= 0 {
                region.prompt_row = new_prompt_row as usize;
                if let Some(ref mut row) = region.command_row {
                    *row = (*row as i32 - scroll_amount).max(0) as usize;
                }
                if let Some(ref mut row) = region.output_row {
                    *row = (*row as i32 - scroll_amount).max(0) as usize;
                }
                if let Some(ref mut row) = region.finish_row {
                    *row = (*row as i32 - scroll_amount).max(0) as usize;
                }
            } else {
                self.current_region = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_tracking() {
        let mut tracker = PromptTracker::new();

        // Simulate: prompt at row 0, command starts at row 0, output at row 1, finish at row 5
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(1);
        tracker.mark_command_finished(5, Some(0));

        // Should have one completed region
        assert_eq!(tracker.regions.len(), 1);
        let region = &tracker.regions[0];
        assert_eq!(region.prompt_row, 0);
        assert_eq!(region.command_row, Some(0));
        assert_eq!(region.output_row, Some(1));
        assert_eq!(region.finish_row, Some(5));
        assert_eq!(region.exit_code, Some(0));
        assert!(region.is_success());
    }

    #[test]
    fn test_prompt_navigation() {
        let mut tracker = PromptTracker::new();

        tracker.mark_prompt_start(0);
        tracker.mark_command_finished(5, Some(0));
        tracker.mark_prompt_start(6);
        tracker.mark_command_finished(10, Some(1));
        tracker.mark_prompt_start(11);

        assert_eq!(tracker.previous_prompt(7), Some(6));
        assert_eq!(tracker.previous_prompt(6), Some(0));
        assert_eq!(tracker.next_prompt(0), Some(6));
        assert_eq!(tracker.next_prompt(6), Some(11));
    }

    #[test]
    fn test_scroll_adjustment() {
        let mut tracker = PromptTracker::new();

        tracker.mark_prompt_start(10);
        tracker.mark_command_start(10);
        tracker.mark_output_start(11);
        tracker.mark_command_finished(15, Some(0));

        // Scroll by 5 lines
        tracker.adjust_for_scroll(5);

        let region = &tracker.regions[0];
        assert_eq!(region.prompt_row, 5);
        assert_eq!(region.output_row, Some(6));
        assert_eq!(region.finish_row, Some(10));
    }
}
