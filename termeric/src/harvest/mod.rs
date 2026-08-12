//! Harvest Mode - Termeric's unique command visualization
//!
//! Provides organic, root-themed UI elements:
//! - Root Cellar: Visual command history timeline
//! - Sprout Actions: Quick context menu on command output
//! - Organic Indicators: Plant-growth animations for long processes

mod cellar;
mod indicators;
pub mod sprouts;

pub use cellar::{CommandEntry, RootCellar};
pub use indicators::GrowthIndicator;
pub use sprouts::SproutMenu;

use termeric_core::CommandRegion;
use termeric_core::shell::PromptTracker;

/// Identity of the terminal that produced a harvested command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarvestSource {
    pub tab_id: usize,
    pub pane_id: Option<u32>,
}

/// Per-terminal cursor and indicator state. Keeping this beside the terminal
/// prevents switching tabs or panes from skipping or duplicating regions.
#[derive(Debug, Default)]
pub struct HarvestTerminalState {
    region_cursor: usize,
    active_prompt_row: Option<usize>,
    pub indicators: Vec<GrowthIndicator>,
}

impl HarvestTerminalState {
    pub fn observe(
        &mut self,
        tracker: &PromptTracker,
        show_indicators: bool,
    ) -> Vec<CommandRegion> {
        if self.region_cursor > tracker.completed_count() {
            self.region_cursor = 0;
        }

        let active_row = tracker
            .active_region()
            .filter(|region| region.output_row.is_some())
            .map(|region| region.prompt_row);
        if active_row != self.active_prompt_row {
            self.active_prompt_row = active_row;
            if let Some(row) = active_row.filter(|_| show_indicators) {
                self.start_command(row);
            }
        }

        let mut completed = Vec::new();
        while self.region_cursor < tracker.completed_count() {
            if let Some(region) = tracker.completed_region(self.region_cursor).cloned() {
                if show_indicators {
                    self.start_command(region.prompt_row);
                    self.complete_command(region.prompt_row, region.exit_code == Some(0));
                }
                if self.active_prompt_row == Some(region.prompt_row) {
                    self.active_prompt_row = None;
                }
                completed.push(region);
            }
            self.region_cursor += 1;
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

    fn start_command(&mut self, prompt_row: usize) {
        if self
            .indicators
            .iter()
            .all(|indicator| indicator.prompt_row != prompt_row)
        {
            self.indicators.push(GrowthIndicator::new(prompt_row));
        }
    }

    fn complete_command(&mut self, prompt_row: usize, success: bool) {
        if let Some(indicator) = self
            .indicators
            .iter_mut()
            .find(|indicator| indicator.prompt_row == prompt_row)
        {
            indicator.complete(success);
        }
    }
}

/// Harvest mode state
#[derive(Debug, Default)]
pub struct HarvestMode {
    /// Root cellar (command history)
    pub cellar: RootCellar,
    /// Active sprout menu (context actions)
    pub sprout_menu: Option<SproutMenu>,
}

impl HarvestMode {
    /// Create a new harvest mode instance with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(100)
    }

    /// Create a harvest mode with a specific cellar history cap.
    pub fn with_capacity(max_history: usize) -> Self {
        Self {
            cellar: RootCellar::with_capacity(max_history),
            sprout_menu: None,
        }
    }

    /// Record a command completion from shell integration
    pub fn record_command(&mut self, source: HarvestSource, region: &CommandRegion) {
        self.cellar.add_from_region(source, region);
    }

    /// Show sprout menu at the given position
    pub fn show_sprout_menu(&mut self, row: usize, col: usize, command: Option<CommandEntry>) {
        self.sprout_menu = Some(SproutMenu::new(row, col, command));
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
    fn cellar_preserves_terminal_ownership() {
        let tracker = one_completed_command();
        let region = tracker.completed_region(0).unwrap();
        let mut harvest = HarvestMode::with_capacity(5);
        let source = HarvestSource {
            tab_id: 7,
            pane_id: Some(3),
        };
        harvest.record_command(source, region);
        assert_eq!(harvest.cellar.latest_for_source(source).unwrap().row, 2);
        assert!(
            harvest
                .cellar
                .latest_for_source(HarvestSource {
                    tab_id: 8,
                    pane_id: None,
                })
                .is_none()
        );
    }
}
