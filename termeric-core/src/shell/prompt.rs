//! Prompt tracking for shell integration
//!
//! Tracks prompt positions and command regions using OSC 133 sequences.

use std::collections::{BTreeSet, HashMap, VecDeque};

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
    pub row: i32,
    /// Type of mark
    pub mark_type: PromptType,
    /// Exit code for CommandFinished marks
    pub exit_code: Option<i32>,
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
#[derive(Debug)]
pub struct PromptTracker {
    /// All recorded marks
    marks: VecDeque<PromptMark>,
    /// Completed command regions
    regions: VecDeque<CommandRegion>,
    /// Current incomplete region being built
    current_region: Option<CommandRegion>,
    next_region_id: u64,
}

impl Default for PromptTracker {
    fn default() -> Self {
        Self {
            marks: VecDeque::new(),
            regions: VecDeque::new(),
            current_region: None,
            next_region_id: 1,
        }
    }
}

impl PromptTracker {
    /// Create a new prompt tracker
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark the start of a prompt (OSC 133;A)
    pub fn mark_prompt_start(&mut self, row: usize) {
        // A new prompt is also a recovery boundary. Only a region that reached
        // output is a real command; prompt-only/empty cycles are discarded.
        self.finish_or_discard_current(bounded_row(row));

        self.add_mark(PromptMark {
            row: bounded_row(row),
            mark_type: PromptType::PromptStart,
            exit_code: None,
        });

        let id = self.next_region_id;
        self.next_region_id = self.next_region_id.saturating_add(1);
        self.current_region = Some(CommandRegion {
            id,
            content_available: true,
            prompt_row: bounded_row(row),
            command_row: None,
            output_row: None,
            finish_row: None,
            exit_code: None,
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
            exit_code: None,
        });

        if let Some(ref mut region) = self.current_region {
            region.command_row = Some(bounded_row(row));
        }
        true
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
            exit_code: None,
        });

        if let Some(ref mut region) = self.current_region {
            region.output_row = Some(bounded_row(row));
        }
        true
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
            exit_code,
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

    /// Get all prompt rows
    pub fn all_prompts(&self) -> impl Iterator<Item = i32> + '_ {
        self.marks
            .iter()
            .filter(|m| m.mark_type == PromptType::PromptStart)
            .map(|m| m.row)
    }

    /// Get the command region containing a given row
    pub fn command_at(&self, row: i32) -> Option<&CommandRegion> {
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
            if region.content_available && region_overlaps(region, start_row, end_row) {
                region.content_available = false;
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

    /// Clear all tracking data
    pub fn clear(&mut self) {
        self.marks.clear();
        self.regions.clear();
        self.current_region = None;
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
    region.content_available &= !(first < bottom && last >= bottom);
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
    if !region.content_available {
        return;
    }
    let Some(prompt_row) = rows.get(&region.prompt_row).copied() else {
        region.content_available = false;
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
