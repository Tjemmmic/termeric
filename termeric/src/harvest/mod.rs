//! Harvest mode: what Termeric shows of the commands that shell integration
//! (OSC 133) reports.
//!
//! - Root Cellar: the list of recent commands and how each one ended
//! - Sprout actions: the menu of actions on one command
//! - Growth indicators: a marker beside a prompt while its command runs,
//!   and briefly after it ends

mod cellar;
mod indicators;
pub mod sprouts;

pub use cellar::{CellarAction, CellarLayout, CommandEntry, CommandHealth, RootCellar};
pub use cellar::{MAX_HARVEST_COMMAND_BYTES, MAX_HARVEST_OUTPUT_BYTES};
pub use indicators::{GrowthIndicator, GrowthPhase};
pub use sprouts::{SproutAvailability, SproutMenu, SproutMenuLayout};

use termeric_core::CommandRegion;
use termeric_core::shell::PromptTracker;

/// The terminal a command ran in: its tab and its pane there. Pane IDs are
/// never reused within a tab, so this stays exact as the tab splits and its
/// panes close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarvestSource {
    pub tab_id: usize,
    pub pane_id: u32,
}

/// Per-terminal cursor and indicator state. Keeping this beside the terminal
/// prevents switching tabs or panes from skipping or duplicating regions.
#[derive(Debug, Default)]
pub struct HarvestTerminalState {
    last_completed_region_id: u64,
    active_region_id: Option<u64>,
    pub indicators: Vec<GrowthIndicator>,
}

impl HarvestTerminalState {
    pub fn observe(
        &mut self,
        tracker: &PromptTracker,
        show_indicators: bool,
    ) -> Vec<CommandRegion> {
        let active = tracker
            .active_region()
            .filter(|region| region.content_available && region.output_row.is_some())
            .map(|region| (region.id, region.prompt_row));
        if active.map(|(id, _)| id) != self.active_region_id {
            self.active_region_id = active.map(|(id, _)| id);
            self.indicators.retain(|indicator| indicator.completed);
            if let Some((id, row)) = active.filter(|_| show_indicators) {
                self.start_command(id, row);
            }
        }

        let mut completed = Vec::new();
        for region in tracker
            .completed_regions_after(self.last_completed_region_id)
            .cloned()
        {
            self.last_completed_region_id = self.last_completed_region_id.max(region.id);
            if show_indicators && region.content_available {
                self.start_command(region.id, region.prompt_row);
                self.complete_command(region.id, region.exit_code == Some(0));
            }
            if self.active_region_id == Some(region.id) {
                self.active_region_id = None;
            }
            completed.push(region);
        }

        if show_indicators {
            self.follow_prompts(tracker);
            for indicator in &mut self.indicators {
                indicator.update();
            }
            self.indicators.retain(|indicator| !indicator.is_finished());
        } else {
            self.indicators.clear();
        }
        completed
    }

    /// Move each indicator to its prompt's row, which output scrolls up and
    /// into scrollback, running or finished. One whose rows are no longer
    /// retained goes with them.
    fn follow_prompts(&mut self, tracker: &PromptTracker) {
        let active = tracker.active_region();
        self.indicators.retain_mut(|indicator| {
            let Some(region) = active
                .filter(|region| region.id == indicator.region_id)
                .or_else(|| tracker.completed_region_by_id(indicator.region_id))
                .filter(|region| region.content_available)
            else {
                return false;
            };
            indicator.prompt_row = region.prompt_row;
            true
        });
    }

    fn start_command(&mut self, region_id: u64, prompt_row: i32) {
        if self
            .indicators
            .iter()
            .all(|indicator| indicator.region_id != region_id)
        {
            self.indicators
                .push(GrowthIndicator::new(region_id, prompt_row));
        }
    }

    fn complete_command(&mut self, region_id: u64, success: bool) {
        if let Some(indicator) = self
            .indicators
            .iter_mut()
            .find(|indicator| indicator.region_id == region_id)
        {
            indicator.complete(success);
        }
    }
}

/// Harvest mode state
#[derive(Debug)]
pub struct HarvestMode {
    /// Root cellar (command history)
    pub cellar: RootCellar,
    /// Active sprout menu (context actions)
    pub sprout_menu: Option<SproutMenu>,
    next_sprout_request_id: u64,
    reduced_motion: bool,
}

impl HarvestMode {
    /// Create a harvest mode with a specific cellar history cap.
    pub fn with_capacity(max_history: usize) -> Self {
        Self {
            cellar: RootCellar::with_capacity(max_history),
            sprout_menu: None,
            next_sprout_request_id: 1,
            reduced_motion: false,
        }
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
        self.cellar.animation.set_reduced_motion(reduced_motion);
        if let Some(menu) = &mut self.sprout_menu {
            menu.set_reduced_motion(reduced_motion);
        }
    }

    /// Record a command completion from shell integration
    pub fn record_command(
        &mut self,
        source: HarvestSource,
        region: &CommandRegion,
        command: impl Into<String>,
        command_truncated: bool,
        output: impl Into<String>,
        output_truncated: bool,
    ) {
        self.cellar.add_from_region(
            source,
            region,
            command,
            command_truncated,
            output,
            output_truncated,
        );
    }

    /// Show sprout menu at the given position
    pub fn show_sprout_menu(
        &mut self,
        row: usize,
        col: usize,
        command: Option<CommandEntry>,
        availability: SproutAvailability,
    ) {
        let request_id = self.next_sprout_request_id;
        self.next_sprout_request_id = self.next_sprout_request_id.saturating_add(1);
        let mut menu = SproutMenu::new(request_id, row, col, command, availability);
        menu.set_reduced_motion(self.reduced_motion);
        self.sprout_menu = Some(menu);
    }

    /// Hide the sprout menu
    pub fn hide_sprout_menu(&mut self) {
        self.sprout_menu = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termeric_core::shell::PromptTracker;

    fn one_completed_command() -> PromptTracker {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(2);
        tracker.mark_command_start(2);
        tracker.mark_output_start(3);
        tracker.mark_command_finished(5, Some(0));
        tracker
    }

    #[test]
    fn terminal_states_consume_regions_once_and_independently() {
        let tracker = one_completed_command();
        let mut first = HarvestTerminalState::default();
        let mut second = HarvestTerminalState::default();

        assert_eq!(first.observe(&tracker, true).len(), 1);
        assert!(first.observe(&tracker, true).is_empty());
        assert_eq!(second.observe(&tracker, true).len(), 1);
    }

    fn indicator_rows(state: &HarvestTerminalState) -> Vec<(u64, i32)> {
        state
            .indicators
            .iter()
            .map(|indicator| (indicator.region_id, indicator.prompt_row))
            .collect()
    }

    #[test]
    fn a_running_commands_indicator_follows_its_prompt_into_scrollback() {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(2);
        tracker.mark_command_start(2);
        tracker.mark_output_start(3);
        let mut state = HarvestTerminalState::default();
        state.observe(&tracker, true);
        let id = tracker.active_region().unwrap().id;
        assert_eq!(indicator_rows(&state), [(id, 2)]);

        // Output scrolls the prompt up past the top of the screen.
        tracker.adjust_for_scroll(5, -100);
        state.observe(&tracker, true);
        assert_eq!(indicator_rows(&state), [(id, -3)]);

        // Once its rows leave the scrollback, the indicator goes too.
        tracker.adjust_for_scroll(100, -100);
        state.observe(&tracker, true);
        assert!(state.indicators.is_empty());
    }

    #[test]
    fn a_finished_commands_indicator_moves_with_its_prompt() {
        let mut tracker = one_completed_command();
        let mut state = HarvestTerminalState::default();
        state.observe(&tracker, true);
        let id = tracker.completed_region(0).unwrap().id;
        assert_eq!(indicator_rows(&state), [(id, 2)]);
        assert!(state.indicators[0].completed);

        tracker.adjust_for_scroll(4, -100);
        state.observe(&tracker, true);
        assert_eq!(indicator_rows(&state), [(id, -2)]);
    }

    #[test]
    fn capacity_eviction_never_replays_retained_regions() {
        let mut tracker = PromptTracker::new();
        for row in 0..1_001 {
            tracker.mark_prompt_start(row);
            tracker.mark_command_start(row);
            tracker.mark_output_start(row);
            tracker.mark_command_finished(row, Some(0));
        }
        let mut state = HarvestTerminalState::default();
        let retained = state.observe(&tracker, false);
        assert_eq!(retained.len(), 1_000);
        let last_id = retained.last().unwrap().id;

        tracker.mark_prompt_start(1_001);
        tracker.mark_command_start(1_001);
        tracker.mark_output_start(1_001);
        tracker.mark_command_finished(1_001, Some(0));
        let next = state.observe(&tracker, false);
        assert_eq!(next.len(), 1);
        assert!(next[0].id > last_id);
    }

    #[test]
    fn completion_is_consumed_once_after_its_live_text_is_evicted() {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(0);
        tracker.mark_command_start(0);
        tracker.mark_output_start(1);

        // A large output can evict the prompt before OSC 133;D arrives. The
        // completion must still reach Harvest exactly once, without granting
        // stale coordinates permission to read replacement grid text.
        tracker.adjust_for_scroll(20, -10);
        tracker.mark_command_finished(2, Some(0));

        let mut state = HarvestTerminalState::default();
        let completed = state.observe(&tracker, true);
        assert_eq!(completed.len(), 1);
        assert!(!completed[0].content_available);
        assert!(state.indicators.is_empty());
        assert!(state.observe(&tracker, true).is_empty());
    }

    #[test]
    fn prompt_only_and_out_of_order_cycles_never_create_harvest_entries() {
        let mut tracker = PromptTracker::new();
        tracker.mark_prompt_start(0);
        tracker.mark_prompt_start(1);
        tracker.mark_command_start(1);
        tracker.mark_command_finished(1, Some(0));
        tracker.mark_output_start(2);

        let mut state = HarvestTerminalState::default();
        assert!(state.observe(&tracker, false).is_empty());

        tracker.mark_prompt_start(3);
        tracker.mark_command_start(3);
        tracker.mark_output_start(4);
        tracker.mark_command_finished(5, Some(0));
        assert_eq!(state.observe(&tracker, false).len(), 1);
        assert!(state.observe(&tracker, false).is_empty());
    }

    #[test]
    fn cellar_preserves_terminal_ownership() {
        let tracker = one_completed_command();
        let region = tracker.completed_region(0).unwrap();
        let mut harvest = HarvestMode::with_capacity(5);
        let sources = [
            HarvestSource {
                tab_id: 7,
                pane_id: 3,
            },
            HarvestSource {
                tab_id: 7,
                pane_id: 4,
            },
            HarvestSource {
                tab_id: 8,
                pane_id: 0,
            },
        ];
        for source in sources {
            harvest.record_command(
                source,
                region,
                "fixture command",
                false,
                "fixture output",
                false,
            );
        }
        for source in sources {
            let entry = harvest.cellar.latest_for_source(source).unwrap();
            assert_eq!(entry.source, source);
            assert_eq!(entry.row, 2);
        }
        assert!(
            harvest
                .cellar
                .latest_for_source(HarvestSource {
                    tab_id: 9,
                    pane_id: 0,
                })
                .is_none()
        );
    }
}
