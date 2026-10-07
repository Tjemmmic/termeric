//! Sprout Actions - Quick context menu for commands
//!
//! Provides quick actions on command output:
//! - Copy output
//! - Re-run command
//! - Search in output
//! - Explain, by asking a running coding agent

use super::cellar::CommandEntry;
use crate::ui::geometry::Bounds;
use std::time::Instant;

/// An action available in the sprout menu
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// Ask a running coding agent to explain the command and its output
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
            Self::CopyOutput => '\u{2398}',    // Copy symbol
            Self::CopyCommand => '\u{2318}',   // Command
            Self::Rerun => '\u{21BB}',         // Clockwise arrow
            Self::SearchOutput => '\u{1F50D}', // Magnifying glass
            Self::ScrollTo => '\u{2193}',      // Down arrow
            Self::Explain => '\u{2139}',       // Info
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

/// Request-scoped facts that determine which actions can be carried out.
///
/// These are captured when the menu opens so every input surface presents the
/// same affordances. The action executor still revalidates live terminal state
/// to handle eviction or tab closure while the menu is visible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SproutAvailability {
    pub command_available: bool,
    pub command_truncated: bool,
    pub output_available: bool,
    pub source_terminal_available: bool,
    pub live_content_available: bool,
    pub source_alternate_screen: bool,
    pub retained_output_truncated: bool,
    /// Agent features are on, so Explain, which hands the command to an
    /// agent, is offered at all.
    pub agent_features: bool,
}

impl SproutAvailability {
    pub fn action_enabled(self, action: SproutAction) -> bool {
        match action {
            SproutAction::CopyOutput => self.output_available,
            SproutAction::CopyCommand | SproutAction::Explain => self.command_available,
            SproutAction::Rerun => {
                self.command_available && !self.command_truncated && self.source_terminal_available
            }
            SproutAction::SearchOutput | SproutAction::ScrollTo => self.live_content_available,
        }
    }

    pub fn action_label(self, action: SproutAction) -> String {
        let suffix = match action {
            SproutAction::CopyOutput if !self.output_available => " (unavailable)",
            SproutAction::CopyCommand | SproutAction::Explain if !self.command_available => {
                " (unavailable)"
            }
            SproutAction::Rerun if !self.command_available => " (unavailable)",
            SproutAction::Rerun if !self.source_terminal_available => " (terminal closed)",
            SproutAction::Rerun | SproutAction::Explain if self.command_truncated => {
                " (truncated command)"
            }
            SproutAction::CopyCommand if self.command_truncated => " (truncated)",
            SproutAction::CopyOutput
                if self.retained_output_truncated && !self.live_content_available =>
            {
                " (truncated)"
            }
            SproutAction::Explain
                if self.retained_output_truncated && !self.live_content_available =>
            {
                " (truncated output)"
            }
            SproutAction::SearchOutput | SproutAction::ScrollTo if self.source_alternate_screen => {
                " (alternate screen)"
            }
            SproutAction::SearchOutput | SproutAction::ScrollTo if !self.live_content_available => {
                " (not retained)"
            }
            _ => "",
        };
        format!("{}{suffix}", action.name())
    }

    pub fn action_description(self, action: SproutAction) -> Option<&'static str> {
        match action {
            SproutAction::Rerun if self.command_truncated => {
                Some("The command text is incomplete; a truncated prefix cannot be rerun")
            }
            SproutAction::CopyCommand if self.command_truncated => {
                Some("Copies only a partial command snapshot, limited to 4 KiB")
            }
            SproutAction::Explain if self.command_truncated => Some(
                "Pastes the partial command text, marked as truncated, into the focused agent terminal, or the only one open",
            ),
            SproutAction::CopyOutput if !self.output_available => {
                Some("No live or retained command output is available")
            }
            SproutAction::CopyCommand | SproutAction::Explain if !self.command_available => {
                Some("Command text is unavailable")
            }
            SproutAction::Rerun if !self.source_terminal_available => {
                Some("The terminal that produced this command is closed")
            }
            SproutAction::Rerun if !self.command_available => Some("Command text is unavailable"),
            SproutAction::SearchOutput | SproutAction::ScrollTo if self.source_alternate_screen => {
                Some("Leave the source terminal's alternate screen to use its primary scrollback")
            }
            SproutAction::SearchOutput | SproutAction::ScrollTo if !self.live_content_available => {
                Some("The command is no longer present in terminal scrollback")
            }
            SproutAction::CopyOutput
                if self.retained_output_truncated && !self.live_content_available =>
            {
                Some("Uses the last 64 KiB of the retained output")
            }
            SproutAction::Explain
                if self.retained_output_truncated && !self.live_content_available =>
            {
                Some(
                    "Pastes the command and the last 64 KiB of its retained output into the focused agent terminal, or the only one open",
                )
            }
            SproutAction::Explain => Some(
                "Pastes the command and its output into the focused agent terminal, or the only one open",
            ),
            _ => None,
        }
    }
}

/// Sprout menu - context menu for commands
#[derive(Debug)]
pub struct SproutMenu {
    /// Identity of this exact visible menu request.
    pub request_id: u64,
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
    /// Availability captured for this exact visible request.
    pub availability: SproutAvailability,
    /// Monotonic start time for the grow-in animation.
    opened_at: Instant,
    reduced_motion: bool,
}

impl SproutMenu {
    /// Create a new sprout menu
    pub fn new(
        request_id: u64,
        row: usize,
        col: usize,
        command: Option<CommandEntry>,
        availability: SproutAvailability,
    ) -> Self {
        let actions = SproutAction::all()
            .iter()
            .copied()
            .filter(|action| *action != SproutAction::Explain || availability.agent_features)
            .collect::<Vec<_>>();
        let selected = actions
            .iter()
            .position(|action| availability.action_enabled(*action))
            .unwrap_or(0);
        Self {
            request_id,
            row,
            col,
            command,
            selected,
            actions,
            availability,
            opened_at: Instant::now(),
            reduced_motion: false,
        }
    }

    /// Move selection up
    pub fn select_up(&mut self) {
        if let Some(index) = (0..self.selected)
            .rev()
            .find(|index| self.action_enabled(self.actions[*index]))
        {
            self.selected = index;
        }
    }

    /// Move selection down
    pub fn select_down(&mut self) {
        if let Some(index) = (self.selected + 1..self.actions.len())
            .find(|index| self.action_enabled(self.actions[*index]))
        {
            self.selected = index;
        }
    }

    /// Get the currently selected action
    pub fn selected_action(&self) -> Option<&SproutAction> {
        self.actions
            .get(self.selected)
            .filter(|action| self.action_enabled(**action))
    }

    pub fn animation_progress(&self) -> f32 {
        if self.reduced_motion {
            1.0
        } else {
            (self.opened_at.elapsed().as_secs_f32() / 0.25).min(1.0)
        }
    }

    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.reduced_motion = reduced_motion;
    }

    /// Get visual opacity based on animation
    pub fn opacity(&self) -> f32 {
        // Ease-out animation
        let t = self.animation_progress();
        1.0 - (1.0 - t).powi(3)
    }

    /// Get menu width in characters
    pub fn width(&self) -> usize {
        self.actions
            .iter()
            .map(|action| self.action_label(*action).chars().count() + 3)
            .max()
            .unwrap_or(10)
    }

    pub fn select(&mut self, action: SproutAction) -> bool {
        if !self.action_enabled(action) {
            return false;
        }
        let Some(index) = self
            .actions
            .iter()
            .position(|candidate| *candidate == action)
        else {
            return false;
        };
        self.selected = index;
        true
    }

    pub fn select_first(&mut self) {
        if let Some(index) = self
            .actions
            .iter()
            .position(|action| self.action_enabled(*action))
        {
            self.selected = index;
        }
    }

    pub fn select_last(&mut self) {
        if let Some(index) = self
            .actions
            .iter()
            .rposition(|action| self.action_enabled(*action))
        {
            self.selected = index;
        }
    }

    pub fn action_enabled(&self, action: SproutAction) -> bool {
        self.availability.action_enabled(action)
    }

    pub fn action_label(&self, action: SproutAction) -> String {
        self.availability.action_label(action)
    }

    pub fn action_description(&self, action: SproutAction) -> Option<&'static str> {
        self.availability.action_description(action)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SproutMenuLayout {
    pub panel: Bounds,
    pub rows: Vec<Bounds>,
    pub padding: f32,
    pub item_height: f32,
}

impl SproutMenuLayout {
    pub fn calculate(
        row: usize,
        col: usize,
        action_count: usize,
        width_chars: usize,
        window_width: f32,
        window_height: f32,
        content_left: f32,
        content_top: f32,
        content_padding: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let padding = cell_width * 0.5;
        let item_height = cell_height * 1.4;
        let width = (width_chars as f32 * cell_width)
            .min((window_width - content_left - content_padding * 2.0).max(0.0));
        let height = action_count as f32 * item_height + padding * 2.0;
        let min_x = content_left + content_padding;
        let preferred_x = min_x + col as f32 * cell_width;
        let preferred_y = content_top + content_padding + row as f32 * cell_height;
        let x = preferred_x
            .min((window_width - width - content_padding).max(min_x))
            .max(min_x);
        let y = preferred_y
            .min((window_height - height - content_padding).max(content_top))
            .max(content_top);
        let panel = Bounds {
            x,
            y,
            width,
            height,
        };
        let rows = (0..action_count)
            .map(|index| Bounds {
                x: x + 2.0,
                y: y + padding + index as f32 * item_height,
                width: (width - 4.0).max(0.0),
                height: item_height,
            })
            .collect();
        Self {
            panel,
            rows,
            padding,
            item_height,
        }
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<usize> {
        self.rows.iter().position(|row| row.contains(x, y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hit_test_finds_the_drawn_rows() {
        let layout =
            SproutMenuLayout::calculate(0, 0, 2, 10, 400.0, 300.0, 0.0, 20.0, 10.0, 10.0, 10.0);
        let first = layout.rows[0];
        let second = layout.rows[1];
        assert_eq!(layout.hit_test(first.x + 1.0, first.y + 1.0), Some(0));
        assert_eq!(layout.hit_test(second.x + 1.0, second.y + 1.0), Some(1));
        assert_eq!(layout.hit_test(1.0, 1.0), None);
    }

    #[test]
    fn truncated_command_affordances_refuse_rerun_and_disclose_partial_copy_and_explain() {
        let availability = SproutAvailability {
            command_available: true,
            command_truncated: true,
            source_terminal_available: true,
            ..Default::default()
        };
        assert!(!availability.action_enabled(SproutAction::Rerun));
        assert_eq!(
            availability.action_label(SproutAction::Rerun),
            "Re-run (truncated command)"
        );
        assert!(
            availability
                .action_description(SproutAction::Rerun)
                .unwrap()
                .contains("incomplete")
        );
        for action in [SproutAction::CopyCommand, SproutAction::Explain] {
            assert!(availability.action_enabled(action));
            assert!(availability.action_label(action).contains("truncated"));
            assert!(
                availability
                    .action_description(action)
                    .unwrap()
                    .contains("partial")
            );
        }
    }

    #[test]
    fn test_sprout_menu_navigation() {
        let availability = SproutAvailability {
            command_available: true,
            command_truncated: false,
            output_available: true,
            source_terminal_available: true,
            live_content_available: true,
            source_alternate_screen: false,
            retained_output_truncated: false,
            agent_features: true,
        };
        let mut menu = SproutMenu::new(1, 0, 0, None, availability);

        assert_eq!(menu.selected, 0);
        menu.select_down();
        assert_eq!(menu.selected, 1);
        menu.select_up();
        assert_eq!(menu.selected, 0);
        menu.select_up(); // Should not go below 0
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn navigation_and_selection_skip_actions_that_need_evicted_live_text() {
        let availability = SproutAvailability {
            command_available: true,
            command_truncated: false,
            output_available: true,
            source_terminal_available: true,
            live_content_available: false,
            source_alternate_screen: false,
            retained_output_truncated: true,
            agent_features: true,
        };
        let mut menu = SproutMenu::new(1, 0, 0, None, availability);
        assert_eq!(
            menu.action_label(SproutAction::CopyOutput),
            "Copy output (truncated)"
        );
        assert_eq!(
            menu.action_label(SproutAction::SearchOutput),
            "Search (not retained)"
        );
        assert!(!menu.select(SproutAction::SearchOutput));
        menu.select(SproutAction::Rerun);
        menu.select_down();
        assert_eq!(menu.selected_action(), Some(&SproutAction::Explain));
        menu.select_up();
        assert_eq!(menu.selected_action(), Some(&SproutAction::Rerun));
    }

    #[test]
    fn explain_is_left_out_while_agent_features_are_off() {
        let on = SproutAvailability {
            command_available: true,
            agent_features: true,
            ..SproutAvailability::default()
        };
        let menu = SproutMenu::new(1, 0, 0, None, on);
        assert!(menu.actions.contains(&SproutAction::Explain));

        let off = SproutAvailability {
            agent_features: false,
            ..on
        };
        let mut menu = SproutMenu::new(1, 0, 0, None, off);
        assert!(!menu.actions.contains(&SproutAction::Explain));
        assert!(!menu.select(SproutAction::Explain));
        assert_eq!(menu.actions.len(), SproutAction::all().len() - 1);
    }

    #[test]
    fn test_sprout_action_names() {
        assert_eq!(SproutAction::CopyOutput.name(), "Copy output");
        assert_eq!(SproutAction::Rerun.name(), "Re-run");
    }

    #[test]
    fn layout_clamps_to_the_window_and_drives_hit_testing() {
        let layout =
            SproutMenuLayout::calculate(200, 200, 6, 20, 320.0, 240.0, 80.0, 40.0, 8.0, 8.0, 16.0);
        assert!(layout.panel.x >= 88.0);
        assert!(layout.panel.y >= 40.0);
        assert!(layout.panel.x + layout.panel.width <= 320.0);
        assert!(layout.panel.y + layout.panel.height <= 240.0);
        let row = layout.rows[3];
        assert_eq!(layout.hit_test(row.x + 1.0, row.y + 1.0), Some(3));
    }

    #[test]
    fn reduced_motion_skips_the_grow_in_transition() {
        let mut menu = SproutMenu::new(1, 0, 0, None, SproutAvailability::default());
        menu.set_reduced_motion(true);
        assert_eq!(menu.animation_progress(), 1.0);
        assert_eq!(menu.opacity(), 1.0);
    }
}
