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
    /// OSC 133 format: `ESC ] 133 ; <type> [; <data>] ST`
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
                if !self.prompt_tracker.mark_command_start(cursor_row) {
                    log::trace!("Ignoring out-of-order or duplicate OSC 133 B");
                }
            }
            "C" => {
                // Output start (command end)
                log::trace!("OSC 133 C: Output start at row {}", cursor_row);
                if !self.prompt_tracker.mark_output_start(cursor_row) {
                    log::trace!("Ignoring out-of-order or duplicate OSC 133 C");
                }
            }
            "D" => {
                // Command finished with optional exit code
                let exit_code = params
                    .get(1)
                    .and_then(|value| std::str::from_utf8(value).ok())
                    .and_then(|value| value.parse::<u8>().ok())
                    .map(i32::from);
                log::trace!(
                    "OSC 133 D: Command finished at row {}, exit code: {:?}",
                    cursor_row,
                    exit_code
                );
                if !self
                    .prompt_tracker
                    .mark_command_finished(cursor_row, exit_code)
                {
                    log::trace!("Ignoring out-of-order or duplicate OSC 133 D");
                }
            }
            _ => {
                log::trace!("OSC 133: Unknown type '{}'", type_str);
            }
        }
    }

    /// Get the previous prompt row from the given row
    pub fn previous_prompt(&self, from_row: i32) -> Option<i32> {
        self.prompt_tracker.previous_prompt(from_row)
    }

    /// Get the next prompt row from the given row
    pub fn next_prompt(&self, from_row: i32) -> Option<i32> {
        self.prompt_tracker.next_prompt(from_row)
    }

    /// Get the command region containing a given row
    pub fn command_at(&self, row: i32) -> Option<&CommandRegion> {
        self.prompt_tracker.command_at(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(shell: &mut ShellIntegration, status: &[u8], row: usize) {
        shell.handle_osc_133(&[b"A"], row);
        shell.handle_osc_133(&[b"B"], row);
        shell.handle_osc_133(&[b"C"], row + 1);
        shell.handle_osc_133(&[b"D", status], row + 2);
    }

    #[test]
    fn exit_status_is_bounded_and_malformed_status_still_closes_the_command() {
        let mut shell = ShellIntegration::new();
        complete(&mut shell, b"0", 0);
        complete(&mut shell, b"255", 3);
        for (index, status) in [b"256".as_slice(), b"-1", b"nope", b"999999999999999999"]
            .into_iter()
            .enumerate()
        {
            complete(&mut shell, status, 6 + index * 3);
        }

        let statuses = (0..shell.prompt_tracker.completed_count())
            .map(|index| {
                shell
                    .prompt_tracker
                    .completed_region(index)
                    .unwrap()
                    .exit_code
            })
            .collect::<Vec<_>>();
        assert_eq!(statuses, vec![Some(0), Some(255), None, None, None, None]);
        assert!(shell.prompt_tracker.active_region().is_none());
    }

    #[test]
    fn malformed_sequences_recover_at_the_next_valid_prompt() {
        let mut shell = ShellIntegration::new();
        for parameters in [
            vec![b"B".as_slice()],
            vec![b"C".as_slice()],
            vec![b"D".as_slice(), b"0"],
        ] {
            shell.handle_osc_133(&parameters, 0);
        }
        shell.handle_osc_133(&[b"A"], 1);
        shell.handle_osc_133(&[b"B"], 1);
        shell.handle_osc_133(&[b"B"], 1);
        shell.handle_osc_133(&[b"D", b"0"], 1);
        assert_eq!(shell.prompt_tracker.completed_count(), 0);

        complete(&mut shell, b"9", 2);
        assert_eq!(shell.prompt_tracker.completed_count(), 1);
        assert_eq!(
            shell.prompt_tracker.completed_region(0).unwrap().exit_code,
            Some(9)
        );
    }
}
