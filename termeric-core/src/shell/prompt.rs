//! Prompt tracking for shell integration
//!
//! Tracks prompt positions and command regions using OSC 133 sequences.

use std::collections::{BTreeSet, HashMap, VecDeque};

/// Maximum number of prompts to track
const MAX_PROMPTS: usize = 1000;

/// The most bytes of command text a region keeps.
pub const MAX_COMMAND_TEXT_BYTES: usize = 4 * 1024;

/// Type of prompt mark
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptType {
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
pub(crate) struct PromptMark {
    /// Row where this mark occurred
    pub row: i32,
    /// Type of mark
    pub mark_type: PromptType,
}

/// A complete command region with prompt, command, and output
#[derive(Debug, Clone)]
pub struct CommandRegion {
    /// Stable terminal-local identity. IDs increase monotonically even when
    /// bounded history evicts older regions.
    pub id: u64,
    /// Whether this region's row coordinates still refer to retained grid
    /// content. Completed identity survives after its text is evicted.
    pub content_available: bool,
    /// Whether the rows still on the grid hold this region's content, even
    /// when its top has scrolled out of history: only an erase, a reflow, or
    /// a scroll region splitting it clears this, so the rest of a long
    /// command's output can still be read.
    pub tail_retained: bool,
    /// Row where the prompt started
    pub prompt_row: i32,
    /// Row where command input started
    pub command_row: Option<i32>,
    /// Row where output started
    pub output_row: Option<i32>,
    /// Row where command finished
    pub finish_row: Option<i32>,
    /// Exit code
    pub exit_code: Option<i32>,
    /// The command as the shell showed it when output started: the text from
    /// the command-start mark to the output-start mark, with soft wraps
    /// joined and surrounding blanks dropped. Empty until output starts.
    pub command_text: String,
    /// Whether `command_text` is missing some of the command: cut at
    /// [`MAX_COMMAND_TEXT_BYTES`], or its start was lost before output began.
    pub command_truncated: bool,
}

impl CommandRegion {
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
#[derive(Debug)]
pub struct PromptTracker {
    /// All recorded marks
    marks: VecDeque<PromptMark>,
    /// Completed command regions
    regions: VecDeque<CommandRegion>,
    /// Current incomplete region being built
    current_region: Option<CommandRegion>,
    next_region_id: u64,
    /// Whether a prompt-start mark ever arrived. It outlives `clear()` so a
    /// shell the user integrated by hand is still recognized after a reset.
    prompt_marks_seen: bool,
    /// Column of the current region's command-start mark, on its
    /// `command_row`. Rows follow scrolling here; the terminal keeps the
    /// column right across a reflow until output starts.
    command_start_column: Option<usize>,
}

impl Default for PromptTracker {
    fn default() -> Self {
        Self {
            marks: VecDeque::new(),
            regions: VecDeque::new(),
            current_region: None,
            next_region_id: 1,
            prompt_marks_seen: false,
            command_start_column: None,
        }
    }
}

impl PromptTracker {
    /// Create a new prompt tracker
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
    ///
    /// Returns the mark the tracker accepted, if any.
    pub(crate) fn handle_osc_133(
        &mut self,
        params: &[&[u8]],
        cursor_row: usize,
    ) -> Option<PromptType> {
        if params.is_empty() {
            return None;
        }

        // Parse the type from the first parameter (after the 133)
        let type_str = std::str::from_utf8(params[0]).unwrap_or("");

        match type_str {
            "A" => {
                // Prompt start
                log::trace!("OSC 133 A: Prompt start at row {}", cursor_row);
                self.mark_prompt_start(cursor_row);
                Some(PromptType::PromptStart)
            }
            "B" => {
                // Command start (prompt end)
                log::trace!("OSC 133 B: Command start at row {}", cursor_row);
                if self.mark_command_start(cursor_row) {
                    Some(PromptType::CommandStart)
                } else {
                    log::trace!("Ignoring out-of-order or duplicate OSC 133 B");
                    None
                }
            }
            "C" => {
                // Output start (command end)
                log::trace!("OSC 133 C: Output start at row {}", cursor_row);
                if self.mark_output_start(cursor_row) {
                    Some(PromptType::OutputStart)
                } else {
                    log::trace!("Ignoring out-of-order or duplicate OSC 133 C");
                    None
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
                if self.mark_command_finished(cursor_row, exit_code) {
                    Some(PromptType::CommandFinished)
                } else {
                    log::trace!("Ignoring out-of-order or duplicate OSC 133 D");
                    None
                }
            }
            _ => {
                log::trace!("OSC 133: Unknown type '{}'", type_str);
                None
            }
        }
    }

    /// Mark the start of a prompt (OSC 133;A)
    pub fn mark_prompt_start(&mut self, row: usize) {
        self.prompt_marks_seen = true;
        // A new prompt is also a recovery boundary. Only a region that reached
        // output is a real command; prompt-only/empty cycles are discarded.
        self.finish_or_discard_current(bounded_row(row));
        self.command_start_column = None;

        self.add_mark(PromptMark {
            row: bounded_row(row),
            mark_type: PromptType::PromptStart,
        });

        let id = self.next_region_id;
        self.next_region_id = self.next_region_id.saturating_add(1);
        self.current_region = Some(CommandRegion {
            id,
            content_available: true,
            tail_retained: true,
            prompt_row: bounded_row(row),
            command_row: None,
            output_row: None,
            finish_row: None,
            exit_code: None,
            command_text: String::new(),
            command_truncated: false,
        });
    }

    /// Mark the start of command input (OSC 133;B)
    pub fn mark_command_start(&mut self, row: usize) -> bool {
        let Some(region) = &self.current_region else {
            return false;
        };
        if region.command_row.is_some() || region.output_row.is_some() {
            return false;
        }
        self.add_mark(PromptMark {
            row: bounded_row(row),
            mark_type: PromptType::CommandStart,
        });

        if let Some(ref mut region) = self.current_region {
            region.command_row = Some(bounded_row(row));
        }
        true
    }

    /// Record the column of the current region's command-start mark.
    pub(crate) fn set_command_start_column(&mut self, column: usize) {
        self.command_start_column = Some(column);
    }

    /// Where the current command's text begins, as (row, column), while the
    /// shell has marked the command's start but not its output yet and the
    /// text is still on the grid.
    pub(crate) fn pending_command_start(&self) -> Option<(i32, usize)> {
        let region = self.current_region.as_ref()?;
        if !region.content_available || region.output_row.is_some() {
            return None;
        }
        Some((region.command_row?, self.command_start_column?))
    }

    /// Move the pending command start after a reflow, or drop it when its
    /// cells did not survive, so its text is never read from the wrong place.
    pub(crate) fn set_pending_command_start(&mut self, start: Option<(i32, usize)>) {
        match start {
            Some((row, column)) => {
                if let Some(region) = &mut self.current_region {
                    region.command_row = Some(row);
                }
                self.command_start_column = Some(column);
            }
            None => self.command_start_column = None,
        }
    }

    /// Store the current region's command text, read when its output began.
    pub(crate) fn record_command_text(&mut self, text: String, truncated: bool) {
        if let Some(region) = &mut self.current_region {
            region.command_text = text;
            region.command_truncated = truncated;
        }
    }

    /// Mark the start of command output (OSC 133;C)
    pub fn mark_output_start(&mut self, row: usize) -> bool {
        let Some(region) = &self.current_region else {
            return false;
        };
        if region.command_row.is_none() || region.output_row.is_some() {
            return false;
        }
        self.add_mark(PromptMark {
            row: bounded_row(row),
            mark_type: PromptType::OutputStart,
        });

        if let Some(ref mut region) = self.current_region {
            region.output_row = Some(bounded_row(row));
        }
        true
    }

    /// Get the command region containing a given row
    #[cfg(test)]
    pub(crate) fn command_at(&self, row: i32) -> Option<&CommandRegion> {
        // Check current region first
        if let Some(ref region) = self.current_region {
            let end = region.finish_row.unwrap_or(i32::MAX);
            if region.content_available && row >= region.prompt_row && row <= end {
                return self.current_region.as_ref();
            }
        }

        // Search completed regions
        self.regions.iter().rev().find(|region| {
            let end = region.finish_row.unwrap_or(i32::MAX);
            region.content_available && row >= region.prompt_row && row <= end
        })
    }

    /// Mark the command as finished (OSC 133;D)
    pub fn mark_command_finished(&mut self, row: usize, exit_code: Option<i32>) -> bool {
        if self
            .current_region
            .as_ref()
            .is_none_or(|region| region.output_row.is_none())
        {
            // D closes a prompt-only/empty cycle so the next valid sequence can
            // recover, but it must not create a ghost command region or mark.
            self.current_region = None;
            return false;
        }
        self.add_mark(PromptMark {
            row: bounded_row(row),
            mark_type: PromptType::CommandFinished,
        });

        if let Some(ref mut region) = self.current_region {
            region.finish_row = Some(bounded_row(row));
            region.exit_code = exit_code;
        }

        self.finish_or_discard_current(bounded_row(row));
        true
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

    /// Complete a command that reached output, or discard an empty/incomplete
    /// prompt cycle. A new A can recover a missing D with an unknown status.
    fn finish_or_discard_current(&mut self, finish_row: i32) {
        let Some(mut region) = self.current_region.take() else {
            return;
        };
        if region.output_row.is_none() {
            return;
        }
        region.finish_row.get_or_insert(finish_row);
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

    /// Get the previous prompt row from the given position
    pub fn previous_prompt(&self, from_row: i32) -> Option<i32> {
        self.marks
            .iter()
            .rev()
            .find(|m| m.mark_type == PromptType::PromptStart && m.row < from_row)
            .map(|m| m.row)
    }

    /// Get the next prompt row from the given position
    pub fn next_prompt(&self, from_row: i32) -> Option<i32> {
        self.marks
            .iter()
            .find(|m| m.mark_type == PromptType::PromptStart && m.row > from_row)
            .map(|m| m.row)
    }

    /// Prompt row of the nearest failed command (non-zero exit) above
    /// `from_row` whose text is still retained.
    pub(crate) fn previous_failed_prompt(&self, from_row: i32) -> Option<i32> {
        self.failed_prompts().rev().find(|row| *row < from_row)
    }

    /// Prompt row of the nearest failed command below `from_row`.
    pub(crate) fn next_failed_prompt(&self, from_row: i32) -> Option<i32> {
        self.failed_prompts().find(|row| *row > from_row)
    }

    fn failed_prompts(&self) -> impl DoubleEndedIterator<Item = i32> + '_ {
        self.regions
            .iter()
            .filter(|region| region.content_available && region.is_failure())
            .map(|region| region.prompt_row)
    }

    /// Get all prompt rows
    pub fn all_prompts(&self) -> impl Iterator<Item = i32> + '_ {
        self.marks
            .iter()
            .filter(|m| m.mark_type == PromptType::PromptStart)
            .map(|m| m.row)
    }

    /// Resolve one exact completed region without relying on mutable row
    /// coordinates or deque position.
    pub fn completed_region_by_id(&self, id: u64) -> Option<&CommandRegion> {
        self.regions.iter().find(|region| region.id == id)
    }

    /// Iterate retained completed regions newer than a previously observed
    /// monotonic identity. Capacity eviction cannot make these replay.
    pub fn completed_regions_after(&self, id: u64) -> impl Iterator<Item = &CommandRegion> {
        self.regions.iter().filter(move |region| region.id > id)
    }

    /// Unique row coordinates that must follow a grid reflow for prompt
    /// navigation and exact region lookup to remain valid.
    pub(crate) fn tracked_rows(&self) -> Vec<i32> {
        let mut rows = BTreeSet::new();
        for mark in &self.marks {
            rows.insert(mark.row);
        }
        for region in self
            .regions
            .iter()
            .chain(self.current_region.iter())
            .filter(|region| region.content_available)
        {
            rows.insert(region.prompt_row);
            rows.extend(region.command_row);
            rows.extend(region.output_row);
            rows.extend(region.finish_row);
        }
        rows.into_iter().collect()
    }

    /// Apply a grid-reflow row map. A region whose boundaries cannot all be
    /// mapped keeps its stable identity but is no longer allowed to resolve
    /// live terminal text.
    pub(crate) fn remap_rows(&mut self, rows: &HashMap<i32, i32>) {
        self.marks.retain_mut(|mark| {
            let Some(row) = rows.get(&mark.row) else {
                return false;
            };
            mark.row = *row;
            true
        });
        for region in self
            .regions
            .iter_mut()
            .chain(self.current_region.iter_mut())
        {
            remap_region(region, rows);
        }
    }

    /// Retire semantic marks and live region authority for erased grid rows.
    /// Completed region identity remains available to one-shot consumers, but
    /// erased coordinates can no longer resolve terminal text.
    pub(crate) fn retire_content_in_rows(&mut self, start_row: i32, end_row: i32) {
        if end_row < start_row {
            return;
        }
        self.marks
            .retain(|mark| !(start_row..=end_row).contains(&mark.row));
        for region in self
            .regions
            .iter_mut()
            .chain(self.current_region.iter_mut())
        {
            if region_overlaps(region, start_row, end_row) {
                region.content_available = false;
                region.tail_retained = false;
            }
        }
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

    /// Whether the program in this terminal has ever marked a prompt, so its
    /// shell integration is running whatever installed it.
    pub fn prompt_marks_seen(&self) -> bool {
        self.prompt_marks_seen
    }

    /// Clear every tracked mark and region
    pub fn clear(&mut self) {
        self.marks.clear();
        self.regions.clear();
        self.current_region = None;
        self.command_start_column = None;
    }

    /// Adjust row numbers after scrolling
    ///
    /// Call this after a full-screen primary-buffer scroll. Coordinates move
    /// into negative scrollback rows. Once their prompt boundary is older than
    /// retained history, regions keep their identity but lose live-content
    /// authority.
    pub fn adjust_for_scroll(&mut self, scroll_amount: i32, oldest_retained_row: i32) {
        // Adjust marks
        self.marks.retain_mut(|mark| {
            mark.row = mark.row.saturating_sub(scroll_amount);
            mark.row >= oldest_retained_row
        });

        // Adjust regions
        for region in &mut self.regions {
            shift_region(region, scroll_amount);
            region.content_available &= region.prompt_row >= oldest_retained_row;
        }

        // Adjust current region
        if let Some(ref mut region) = self.current_region {
            shift_region(region, scroll_amount);
            region.content_available &= region.prompt_row >= oldest_retained_row;
        }
    }

    /// Follow a single archived row from a full-width, top-origin region.
    /// History and rows above `bottom` move together; the footer does not.
    /// Regions spanning both cannot safely resolve one contiguous text range.
    /// The active cursor bounds output that has not yet received an OSC 133 D.
    pub(crate) fn adjust_for_archival_region(
        &mut self,
        bottom: usize,
        oldest_retained_row: i32,
        cursor_row: usize,
    ) {
        let bottom = bounded_row(bottom);
        self.marks.retain_mut(|mark| {
            if mark.row < bottom {
                mark.row = mark.row.saturating_sub(1);
            }
            mark.row >= oldest_retained_row
        });
        for region in &mut self.regions {
            shift_archival_region(region, bottom, oldest_retained_row, None);
        }
        if let Some(region) = &mut self.current_region {
            shift_archival_region(
                region,
                bottom,
                oldest_retained_row,
                Some(bounded_row(cursor_row)),
            );
        }
    }
}

fn shift_archival_region(
    region: &mut CommandRegion,
    bottom: i32,
    oldest_retained_row: i32,
    active_cursor: Option<i32>,
) {
    let mut first = region.prompt_row;
    let mut last = first;
    for row in [
        region.command_row,
        region.output_row,
        region.finish_row,
        active_cursor,
    ]
    .into_iter()
    .flatten()
    {
        first = first.min(row);
        last = last.max(row);
    }
    if first < bottom && last >= bottom {
        region.content_available = false;
        region.tail_retained = false;
    }
    for row in std::iter::once(&mut region.prompt_row).chain(
        [
            &mut region.command_row,
            &mut region.output_row,
            &mut region.finish_row,
        ]
        .into_iter()
        .flatten(),
    ) {
        if *row < bottom {
            *row = row.saturating_sub(1);
        }
        region.content_available &= *row >= oldest_retained_row;
    }
}

fn bounded_row(row: usize) -> i32 {
    i32::try_from(row).unwrap_or(i32::MAX)
}

fn shift_region(region: &mut CommandRegion, amount: i32) {
    region.prompt_row = region.prompt_row.saturating_sub(amount);
    for row in [
        &mut region.command_row,
        &mut region.output_row,
        &mut region.finish_row,
    ]
    .into_iter()
    .flatten()
    {
        *row = row.saturating_sub(amount);
    }
}

fn remap_region(region: &mut CommandRegion, rows: &HashMap<i32, i32>) {
    // A reflow maps only whole regions; the rows of one whose top was gone
    // no longer refer to its text.
    if !region.content_available {
        region.tail_retained = false;
        return;
    }
    let Some(prompt_row) = rows.get(&region.prompt_row).copied() else {
        region.content_available = false;
        region.tail_retained = false;
        return;
    };
    let remap_optional = |row: Option<i32>| match row {
        Some(row) => rows.get(&row).copied().map(Some),
        None => Some(None),
    };
    let (Some(command_row), Some(output_row), Some(finish_row)) = (
        remap_optional(region.command_row),
        remap_optional(region.output_row),
        remap_optional(region.finish_row),
    ) else {
        region.content_available = false;
        region.tail_retained = false;
        return;
    };
    region.prompt_row = prompt_row;
    region.command_row = command_row;
    region.output_row = output_row;
    region.finish_row = finish_row;
}

fn region_overlaps(region: &CommandRegion, start_row: i32, end_row: i32) -> bool {
    let mut first = region.prompt_row;
    let mut last = region.finish_row.unwrap_or(i32::MAX);
    for row in [region.command_row, region.output_row, region.finish_row]
        .into_iter()
        .flatten()
    {
        first = first.min(row);
        last = last.max(row);
    }
    first <= end_row && last >= start_row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(tracker: &mut PromptTracker, status: &[u8], row: usize) {
        tracker.handle_osc_133(&[b"A"], row);
        tracker.handle_osc_133(&[b"B"], row);
        tracker.handle_osc_133(&[b"C"], row + 1);
        tracker.handle_osc_133(&[b"D", status], row + 2);
    }

    #[test]
    fn exit_status_is_bounded_and_malformed_status_still_closes_the_command() {
        let mut tracker = PromptTracker::new();
        complete(&mut tracker, b"0", 0);
        complete(&mut tracker, b"255", 3);
        for (index, status) in [b"256".as_slice(), b"-1", b"nope", b"999999999999999999"]
            .into_iter()
            .enumerate()
        {
            complete(&mut tracker, status, 6 + index * 3);
        }

        let statuses = (0..tracker.completed_count())
            .map(|index| tracker.completed_region(index).unwrap().exit_code)
            .collect::<Vec<_>>();
        assert_eq!(statuses, vec![Some(0), Some(255), None, None, None, None]);
        assert!(tracker.active_region().is_none());
    }

    #[test]
    fn malformed_sequences_recover_at_the_next_valid_prompt() {
        let mut tracker = PromptTracker::new();
        for parameters in [
            vec![b"B".as_slice()],
            vec![b"C".as_slice()],
            vec![b"D".as_slice(), b"0"],
        ] {
            tracker.handle_osc_133(&parameters, 0);
        }
        tracker.handle_osc_133(&[b"A"], 1);
        tracker.handle_osc_133(&[b"B"], 1);
        tracker.handle_osc_133(&[b"B"], 1);
        tracker.handle_osc_133(&[b"D", b"0"], 1);
        assert_eq!(tracker.completed_count(), 0);

        complete(&mut tracker, b"9", 2);
        assert_eq!(tracker.completed_count(), 1);
        assert_eq!(tracker.completed_region(0).unwrap().exit_code, Some(9));
    }

    #[test]
    fn prompt_marks_seen_starts_at_the_first_prompt_and_survives_clear() {
        let mut tracker = PromptTracker::new();
        tracker.handle_osc_133(&[b"B"], 0);
        tracker.handle_osc_133(&[b"D", b"0"], 0);
        assert!(!tracker.prompt_marks_seen());

        tracker.handle_osc_133(&[b"A"], 1);
        assert!(tracker.prompt_marks_seen());
        tracker.clear();
        assert!(tracker.prompt_marks_seen());
    }

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
    }

    #[test]
    fn test_prompt_navigation() {
        let mut tracker = PromptTracker::new();

        tracker.mark_prompt_start(0);
        tracker.mark_prompt_start(6);
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
        tracker.adjust_for_scroll(5, -100);

        let region = &tracker.regions[0];
        assert_eq!(region.prompt_row, 5);
        assert_eq!(region.output_row, Some(6));
        assert_eq!(region.finish_row, Some(10));
    }

    #[test]
    fn failed_command_navigation_skips_successes_and_unfinished_commands() {
        let mut tracker = PromptTracker::new();
        for (prompt, exit) in [(0, Some(0)), (3, Some(2)), (6, Some(0)), (9, Some(127))] {
            tracker.mark_prompt_start(prompt);
            tracker.mark_command_start(prompt);
            tracker.mark_output_start(prompt + 1);
            tracker.mark_command_finished(prompt + 2, exit);
        }
        // A command still running has no exit status and is never a target.
        tracker.mark_prompt_start(12);
        tracker.mark_command_start(12);
        tracker.mark_output_start(13);

        assert_eq!(tracker.previous_failed_prompt(20), Some(9));
        assert_eq!(tracker.previous_failed_prompt(9), Some(3));
        assert_eq!(tracker.previous_failed_prompt(3), None);
        assert_eq!(tracker.next_failed_prompt(-1), Some(3));
        assert_eq!(tracker.next_failed_prompt(3), Some(9));
        assert_eq!(tracker.next_failed_prompt(9), None);
    }

    #[test]
    fn scrolling_preserves_negative_history_coordinates_until_eviction() {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(1);
        tracker.mark_command_finished(2, Some(0));
        let id = tracker.completed_region(0).unwrap().id;

        tracker.adjust_for_scroll(3, -10);
        let region = tracker.completed_region_by_id(id).unwrap();
        assert_eq!(region.prompt_row, -3);
        assert_eq!(region.output_row, Some(-2));
        assert_eq!(region.finish_row, Some(-1));
        assert_eq!(tracker.command_at(-2).unwrap().id, id);

        tracker.adjust_for_scroll(8, -10);
        let region = tracker.completed_region_by_id(id).unwrap();
        assert!(!region.content_available);
        assert!(tracker.command_at(region.prompt_row).is_none());
    }

    #[test]
    fn monotonic_ids_do_not_replay_survivors_after_capacity_eviction() {
        let mut tracker = PromptTracker::new();
        for row in 0..=MAX_PROMPTS {
            tracker.mark_prompt_start(row);
            tracker.mark_command_start(row);
            tracker.mark_output_start(row);
            tracker.mark_command_finished(row, Some(0));
        }
        assert_eq!(tracker.completed_count(), MAX_PROMPTS);
        let first_retained = tracker.completed_region(0).unwrap().id;
        let latest = tracker.completed_region(MAX_PROMPTS - 1).unwrap().id;
        assert_eq!(first_retained, 2);
        assert_eq!(latest, (MAX_PROMPTS + 1) as u64);
        assert_eq!(tracker.completed_regions_after(latest - 1).count(), 1);
        assert_eq!(tracker.completed_regions_after(latest).count(), 0);
    }

    #[test]
    fn erased_rows_retire_live_authority_without_losing_completion_identity() {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(1);
        tracker.mark_command_finished(5, Some(0));
        let id = tracker.completed_region(0).unwrap().id;

        // The erased rows lie inside the output even though no boundary mark
        // is located on either row.
        tracker.retire_content_in_rows(2, 3);
        let region = tracker.completed_region_by_id(id).unwrap();
        assert!(!region.content_available);
        assert!(tracker.command_at(2).is_none());
        assert_eq!(tracker.completed_regions_after(0).count(), 1);

        tracker.mark_prompt_start(6);
        tracker.mark_command_start(6);
        tracker.mark_output_start(6);
        tracker.retire_content_in_rows(6, 6);
        tracker.mark_command_finished(7, Some(1));
        let active_completion = tracker.completed_region(1).unwrap();
        assert!(!active_completion.content_available);
        assert_eq!(active_completion.exit_code, Some(1));
    }

    #[test]
    fn lifecycle_rejects_ghosts_duplicates_and_out_of_order_marks() {
        let mut tracker = PromptTracker::new();
        assert!(!tracker.mark_command_start(0));
        assert!(!tracker.mark_output_start(0));
        assert!(!tracker.mark_command_finished(0, Some(0)));
        assert_eq!(tracker.completed_count(), 0);

        // Repeated prompts and an empty A/B/D cycle remain navigation marks,
        // not harvested commands.
        tracker.mark_prompt_start(1);
        tracker.mark_prompt_start(2);
        assert!(tracker.mark_command_start(2));
        assert!(!tracker.mark_command_start(2));
        assert!(!tracker.mark_command_finished(2, Some(0)));
        assert_eq!(tracker.completed_count(), 0);
        assert_eq!(tracker.all_prompts().collect::<Vec<_>>(), vec![1, 2]);

        tracker.mark_prompt_start(3);
        assert!(tracker.mark_command_start(3));
        assert!(tracker.mark_output_start(4));
        assert!(!tracker.mark_output_start(4));
        assert!(tracker.mark_command_finished(5, Some(7)));
        assert!(!tracker.mark_command_finished(5, Some(7)));
        assert_eq!(tracker.completed_count(), 1);
        assert_eq!(tracker.completed_region(0).unwrap().exit_code, Some(7));
    }

    #[test]
    fn new_prompt_recovers_a_missing_finish_only_after_output_started() {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(1);
        let recovered_id = tracker.active_region().unwrap().id;

        tracker.mark_prompt_start(5);
        let recovered = tracker.completed_region_by_id(recovered_id).unwrap();
        assert_eq!(recovered.finish_row, Some(5));
        assert_eq!(recovered.exit_code, None);
        assert!(!recovered.is_running());

        // A prompt with no C is discarded at the next A rather than becoming
        // a forever-running completed command.
        tracker.mark_command_start(5);
        tracker.mark_prompt_start(6);
        assert_eq!(tracker.completed_count(), 1);
    }
}
