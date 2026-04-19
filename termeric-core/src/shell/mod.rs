//! Shell integration for enhanced terminal features
//!
//! Implements OSC 133 for prompt detection and command tracking.

mod prompt;

pub use prompt::{CommandRegion, PromptMark, PromptTracker, PromptType};

/// Shell integration state for a terminal
#[derive(Debug, Default)]
pub struct ShellIntegration {
    /// Prompt tracking
    pub prompt_tracker: PromptTracker,
}

impl ShellIntegration {
    /// Create a new shell integration state
    pub fn new() -> Self {
        Self::default()
    }

    /// Handle OSC 133 sequence
    ///
    /// OSC 133 format: ESC ] 133 ; <type> [; <data>] ST
    ///
    /// Types:
    /// - A: Prompt start (fresh line)
    /// - B: Prompt end / Command start
    /// - C: Command end / Output start
    /// - D: Output end (with optional exit code)
    pub fn handle_osc_133(&mut self, params: &[&[u8]], cursor_row: usize) {
        if params.is_empty() {
            return;
        }

        // Parse the type from the first parameter (after the 133)
        let type_str = std::str::from_utf8(params[0]).unwrap_or("");

        match type_str {
            "A" => {
                // Prompt start
                log::trace!("OSC 133 A: Prompt start at row {}", cursor_row);
                self.prompt_tracker.mark_prompt_start(cursor_row);
            }
            "B" => {
                // Command start (prompt end)
                log::trace!("OSC 133 B: Command start at row {}", cursor_row);
                self.prompt_tracker.mark_command_start(cursor_row);
            }
            "C" => {
                // Output start (command end)
                log::trace!("OSC 133 C: Output start at row {}", cursor_row);
                self.prompt_tracker.mark_output_start(cursor_row);
            }
            "D" => {
                // Command finished with optional exit code
                let exit_code = if params.len() > 1 {
                    std::str::from_utf8(params[1])
                        .ok()
                        .and_then(|s| s.parse().ok())
                } else {
                    None
                };
                log::trace!(
                    "OSC 133 D: Command finished at row {}, exit code: {:?}",
                    cursor_row,
                    exit_code
                );
                self.prompt_tracker
                    .mark_command_finished(cursor_row, exit_code);
            }
            _ => {
                log::trace!("OSC 133: Unknown type '{}'", type_str);
            }
        }
    }

    /// Get the previous prompt row from the given row
    pub fn previous_prompt(&self, from_row: i32) -> Option<usize> {
        self.prompt_tracker.previous_prompt(from_row)
    }

    /// Get the next prompt row from the given row
    pub fn next_prompt(&self, from_row: i32) -> Option<usize> {
        self.prompt_tracker.next_prompt(from_row)
    }

    /// Get the command region containing a given row
    pub fn command_at(&self, row: i32) -> Option<&CommandRegion> {
        self.prompt_tracker.command_at(row)
    }
}
