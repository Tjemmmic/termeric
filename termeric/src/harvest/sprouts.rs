//! Sprout Actions - Quick context menu for commands
//!
//! Provides quick actions on command output:
//! - Copy output
//! - Re-run command
//! - Search in output
//! - Explain (future AI integration)

use super::cellar::CommandEntry;

/// An action available in the sprout menu
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SproutAction {
    /// Copy the command output
    CopyOutput,
    /// Copy the command itself
    CopyCommand,
    /// Re-run the command
    Rerun,
    /// Search within the output
    SearchOutput,
    /// Scroll to this command
    ScrollTo,
    /// Explain the command (future feature)
    Explain,
}

impl SproutAction {
    /// Get the display name for this action
    pub fn name(&self) -> &'static str {
        match self {
            Self::CopyOutput => "Copy output",
            Self::CopyCommand => "Copy command",
            Self::Rerun => "Re-run",
            Self::SearchOutput => "Search",
            Self::ScrollTo => "Scroll to",
            Self::Explain => "Explain",
        }
    }

    /// Get the icon character for this action
    pub fn icon(&self) -> char {
        match self {
            Self::CopyOutput => '\u{2398}',   // Copy symbol
            Self::CopyCommand => '\u{2318}',  // Command
            Self::Rerun => '\u{21BB}',        // Clockwise arrow
            Self::SearchOutput => '\u{1F50D}', // Magnifying glass
            Self::ScrollTo => '\u{2193}',     // Down arrow
            Self::Explain => '\u{2139}',      // Info
        }
    }

    /// Get all available actions
    pub fn all() -> &'static [SproutAction] {
        &[
            Self::CopyOutput,
            Self::CopyCommand,
            Self::Rerun,
            Self::SearchOutput,
            Self::ScrollTo,
            Self::Explain,
        ]
    }
}

/// Sprout menu - context menu for commands
#[derive(Debug)]
pub struct SproutMenu {
    /// Row position
    pub row: usize,
    /// Column position
    pub col: usize,
    /// Associated command entry (if any)
    pub command: Option<CommandEntry>,
    /// Currently selected action index
    pub selected: usize,
    /// Available actions
    pub actions: Vec<SproutAction>,
    /// Animation progress (0.0 to 1.0)
    pub animation: f32,
}

impl SproutMenu {
    /// Create a new sprout menu
    pub fn new(row: usize, col: usize, command: Option<CommandEntry>) -> Self {
        let actions = SproutAction::all().to_vec();
        Self {
            row,
            col,
            command,
            selected: 0,
            actions,
            animation: 0.0,
        }
    }

    /// Move selection up
    pub fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down
    pub fn select_down(&mut self) {
        if self.selected + 1 < self.actions.len() {
            self.selected += 1;
        }
    }

    /// Get the currently selected action
    pub fn selected_action(&self) -> Option<&SproutAction> {
        self.actions.get(self.selected)
    }

    /// Update animation
    pub fn update(&mut self, dt: f32) {
        // Grow animation
        if self.animation < 1.0 {
            self.animation += dt * 4.0; // 0.25 second animation
            self.animation = self.animation.min(1.0);
        }
    }

    /// Get visual opacity based on animation
    pub fn opacity(&self) -> f32 {
        // Ease-out animation
        let t = self.animation;
        1.0 - (1.0 - t).powi(3)
    }

    /// Get menu width in characters
    pub fn width(&self) -> usize {
        self.actions
            .iter()
            .map(|a| a.name().len() + 3) // +3 for icon and spacing
            .max()
            .unwrap_or(10)
    }

    /// Get menu height in lines
    pub fn height(&self) -> usize {
        self.actions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sprout_menu_navigation() {
        let mut menu = SproutMenu::new(0, 0, None);

        assert_eq!(menu.selected, 0);
        menu.select_down();
        assert_eq!(menu.selected, 1);
        menu.select_up();
        assert_eq!(menu.selected, 0);
        menu.select_up(); // Should not go below 0
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn test_sprout_action_names() {
        assert_eq!(SproutAction::CopyOutput.name(), "Copy output");
        assert_eq!(SproutAction::Rerun.name(), "Re-run");
    }
}
