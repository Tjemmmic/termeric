//! Root Cellar - Visual command history
//!
//! Displays commands as "roots" growing from a timeline,
//! with branching for piped commands and color-coding by exit status.

use std::collections::VecDeque;
use std::time::Instant;
use termeric_core::CommandRegion;

/// Maximum number of commands to keep in history
const MAX_COMMANDS: usize = 100;

/// A command entry in the root cellar
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CommandEntry {
    /// Row where the command started
    pub row: usize,
    /// Exit code (None if still running)
    pub exit_code: Option<i32>,
    /// When the command was recorded
    pub timestamp: Instant,
    /// Whether this command was successful
    pub success: bool,
    /// Visual state for animation
    pub visual_state: CommandVisualState,
}

/// Visual state for command entry animation
#[derive(Debug, Clone, Copy, Default)]
pub struct CommandVisualState {
    /// Animation progress (0.0 to 1.0)
    pub progress: f32,
    /// Current opacity
    pub opacity: f32,
    /// "Root depth" for visual effect
    pub depth: f32,
}

impl CommandEntry {
    /// Create a new command entry
    pub fn new(row: usize, exit_code: Option<i32>) -> Self {
        let success = exit_code == Some(0);
        Self {
            row,
            exit_code,
            timestamp: Instant::now(),
            success,
            visual_state: CommandVisualState {
                progress: 0.0,
                opacity: 1.0,
                depth: 0.0,
            },
        }
    }

    /// Get the health of this command (for visual styling)
    /// Returns: Healthy (0), Wilted (1), Running (2)
    pub fn health(&self) -> u8 {
        match self.exit_code {
            Some(0) => 0, // Healthy root
            Some(_) => 1, // Wilted root
            None => 2,    // Still growing
        }
    }

    /// Get color based on health
    /// Returns [r, g, b, a]
    pub fn health_color(&self) -> [f32; 4] {
        match self.health() {
            0 => [0.4, 0.7, 0.3, 1.0], // Healthy green
            1 => [0.7, 0.4, 0.3, 1.0], // Wilted brown
            _ => [0.6, 0.5, 0.4, 0.7], // Growing - muted
        }
    }
}

/// Root Cellar - command history with visual timeline
#[derive(Debug, Default)]
pub struct RootCellar {
    /// Command entries
    entries: VecDeque<CommandEntry>,
    /// Whether the cellar view is expanded
    pub expanded: bool,
    /// Scroll position in the cellar view
    pub scroll: f32,
}

#[allow(dead_code)]
impl RootCellar {
    /// Create a new root cellar
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a command entry from a shell integration region
    pub fn add_from_region(&mut self, region: &CommandRegion) {
        let entry = CommandEntry::new(region.prompt_row, region.exit_code);
        self.add(entry);
    }

    /// Add a command entry
    pub fn add(&mut self, entry: CommandEntry) {
        // Remove oldest if at capacity
        if self.entries.len() >= MAX_COMMANDS {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// Get all entries
    pub fn entries(&self) -> impl Iterator<Item = &CommandEntry> {
        self.entries.iter()
    }

    /// Get the most recent entry
    pub fn latest(&self) -> Option<&CommandEntry> {
        self.entries.back()
    }

    /// Get entry at a specific row
    pub fn at_row(&self, row: usize) -> Option<&CommandEntry> {
        self.entries.iter().find(|e| e.row == row)
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Toggle expanded view
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Update animations (call each frame)
    pub fn update(&mut self, dt: f32) {
        for entry in &mut self.entries {
            // Animate progress towards 1.0
            if entry.visual_state.progress < 1.0 {
                entry.visual_state.progress += dt * 2.0; // 0.5 second animation
                entry.visual_state.progress = entry.visual_state.progress.min(1.0);
            }
        }
    }

    /// Get success rate (percentage of successful commands)
    pub fn success_rate(&self) -> f32 {
        if self.entries.is_empty() {
            return 1.0;
        }

        let completed: Vec<_> = self.entries
            .iter()
            .filter(|e| e.exit_code.is_some())
            .collect();

        if completed.is_empty() {
            return 1.0;
        }

        let successful = completed.iter().filter(|e| e.success).count();
        successful as f32 / completed.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_entry_health() {
        let success = CommandEntry::new(0, Some(0));
        assert_eq!(success.health(), 0);

        let failure = CommandEntry::new(0, Some(1));
        assert_eq!(failure.health(), 1);

        let running = CommandEntry::new(0, None);
        assert_eq!(running.health(), 2);
    }

    #[test]
    fn test_root_cellar_add() {
        let mut cellar = RootCellar::new();
        cellar.add(CommandEntry::new(0, Some(0)));
        cellar.add(CommandEntry::new(5, Some(1)));

        assert_eq!(cellar.len(), 2);
        assert_eq!(cellar.at_row(0).unwrap().exit_code, Some(0));
        assert_eq!(cellar.at_row(5).unwrap().exit_code, Some(1));
    }

    #[test]
    fn test_success_rate() {
        let mut cellar = RootCellar::new();
        cellar.add(CommandEntry::new(0, Some(0)));
        cellar.add(CommandEntry::new(5, Some(0)));
        cellar.add(CommandEntry::new(10, Some(1)));

        // 2 successes out of 3
        assert!((cellar.success_rate() - 0.666).abs() < 0.01);
    }
}
