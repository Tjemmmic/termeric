//! Harvest Mode - Termeric's unique command visualization
//!
//! Provides organic, root-themed UI elements:
//! - Root Cellar: Visual command history timeline
//! - Sprout Actions: Quick context menu on command output
//! - Organic Indicators: Plant-growth animations for long processes

mod cellar;
pub mod sprouts;
mod indicators;

pub use cellar::{RootCellar, CommandEntry};
pub use sprouts::SproutMenu;
pub use indicators::GrowthIndicator;

use termeric_core::CommandRegion;

/// Harvest mode state
#[derive(Debug, Default)]
pub struct HarvestMode {
    /// Root cellar (command history)
    pub cellar: RootCellar,
    /// Active sprout menu (context actions)
    pub sprout_menu: Option<SproutMenu>,
    /// Growth indicators for running commands
    pub indicators: Vec<GrowthIndicator>,
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
            indicators: Vec::new(),
        }
    }

    /// Record a command completion from shell integration
    pub fn record_command(&mut self, region: &CommandRegion) {
        self.cellar.add_from_region(region);
    }

    /// Show sprout menu at the given position
    pub fn show_sprout_menu(&mut self, row: usize, col: usize, command: Option<CommandEntry>) {
        self.sprout_menu = Some(SproutMenu::new(row, col, command));
    }

    /// Hide the sprout menu
    pub fn hide_sprout_menu(&mut self) {
        self.sprout_menu = None;
    }

    /// Start tracking a running command
    pub fn start_command(&mut self, prompt_row: usize) {
        self.indicators.push(GrowthIndicator::new(prompt_row));
    }

    /// Mark a command as completed
    pub fn complete_command(&mut self, prompt_row: usize, success: bool) {
        if let Some(indicator) = self.indicators.iter_mut().find(|i| i.prompt_row == prompt_row) {
            indicator.complete(success);
        }
    }

    /// Update animations (call each frame)
    pub fn update(&mut self, dt: f32) {
        for indicator in &mut self.indicators {
            indicator.update(dt);
        }

        // Remove completed indicators after their animation finishes
        self.indicators.retain(|i| !i.is_finished());
    }

    /// Check if any indicators need rendering
    pub fn has_active_indicators(&self) -> bool {
        !self.indicators.is_empty()
    }
}
