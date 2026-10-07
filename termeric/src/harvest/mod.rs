//! Harvest Mode - Termeric's unique command visualization
//!
//! Provides organic, root-themed UI elements:
//! - Root Cellar: Visual command history timeline
//! - Sprout Actions: Quick context menu on command output
//! - Organic Indicators: Plant-growth animations for long processes

mod cellar;
mod indicators;
pub mod sprouts;

#[cfg(test)]
pub use cellar::MAX_HARVEST_COMMAND_BYTES;
pub use cellar::{CellarAction, CellarLayout, CommandEntry, CommandHealth, RootCellar};
pub use indicators::{GrowthIndicator, GrowthPhase};
pub use sprouts::{SproutAvailability, SproutMenu, SproutMenuLayout};

use termeric_core::CommandRegion;
use termeric_core::shell::PromptTracker;

/// Stable tab-local identity, independent of single/split ownership mode.
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
            .and_then(|region| {
                usize::try_from(region.prompt_row)
                    .ok()
                    .map(|row| (region.id, row))
            });
        if active.map(|(id, _)| id) != self.active_region_id {
            self.active_region_id = active.map(|(id, _)| id);
            self.indicators.retain(|indicator| indicator.completed);
            if let Some((id, row)) = active.filter(|_| show_indicators) {
                self.start_command(id, row);
            }
        } else if let Some((id, row)) = active
            && let Some(indicator) = self
                .indicators
                .iter_mut()
                .find(|indicator| indicator.region_id == id)
        {
            indicator.prompt_row = row;
        }

        let mut completed = Vec::new();
        for region in tracker
            .completed_regions_after(self.last_completed_region_id)
            .cloned()
        {
            self.last_completed_region_id = self.last_completed_region_id.max(region.id);
            if show_indicators && let Ok(row) = usize::try_from(region.prompt_row) {
                self.start_command(region.id, row);
                self.complete_command(region.id, region.exit_code == Some(0));
            }
            if self.active_region_id == Some(region.id) {
                self.active_region_id = None;
            }
            completed.push(region);
        }

        if show_indicators {
            for indicator in &mut self.indicators {
                indicator.update();
            }
            self.indicators.retain(|indicator| !indicator.is_finished());
        } else {
            self.indicators.clear();
        }
        completed
    }

    fn start_command(&mut self, region_id: u64, prompt_row: usize) {
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

impl Default for HarvestMode {
    fn default() -> Self {
        Self::with_capacity(100)
    }
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
