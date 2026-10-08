//! Mouse input handling and coordinate conversion

use std::time::{Duration, Instant};

use termeric_core::{Point, Selection, SelectionType};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// The longest gap between two presses that still makes them a
/// double-click, everywhere Termeric counts clicks.
pub const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

/// Recognizes a second press on the same target within
/// [`DOUBLE_CLICK_INTERVAL`]. A third press starts a new pair.
#[derive(Debug)]
pub struct DoubleClick<T> {
    last: Option<(T, Instant)>,
}

impl<T> Default for DoubleClick<T> {
    fn default() -> Self {
        Self { last: None }
    }
}

impl<T: PartialEq> DoubleClick<T> {
    /// Record a press on `target` at `now`; true when it completes a
    /// double-click.
    pub fn press(&mut self, target: T, now: Instant) -> bool {
        let repeated = self.last.as_ref().is_some_and(|(last, at)| {
            *last == target && now.saturating_duration_since(*at) <= DOUBLE_CLICK_INTERVAL
        });
        self.last = (!repeated).then_some((target, now));
        repeated
    }

    /// Forget the last press, so the next one starts a new pair.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Mouse input state tracker
pub struct MouseState {
    /// Current selection being made
    pub selection: Option<Selection>,
    /// Number of rapid clicks (for double/triple click detection)
    pub click_count: u32,
    /// Time of last click (for double/triple click detection)
    pub last_click_time: Instant,
    /// Position of last click
    pub last_click_pos: Option<Point>,
}

impl Default for MouseState {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseState {
    pub fn new() -> Self {
        Self {
            selection: None,
            click_count: 0,
            last_click_time: Instant::now(),
            last_click_pos: None,
        }
    }

    /// Handle mouse button press
    pub fn handle_press(
        &mut self,
        button: MouseButton,
        position: Point,
        modifiers: ModifiersState,
    ) -> Option<Selection> {
        if button != MouseButton::Left {
            return None;
        }

        // Check for multi-click (double/triple).
        // Proximity is tight — same row, same column (no fuzz) — so rapid
        // clicks on adjacent words don't get merged into false triple-clicks.
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_click_time);
        let same_position = self
            .last_click_pos
            .map(|p| p.row == position.row && p.col == position.col)
            .unwrap_or(false);

        if elapsed < DOUBLE_CLICK_INTERVAL && same_position {
            self.click_count = (self.click_count % 3) + 1;
        } else {
            self.click_count = 1;
        }

        self.last_click_time = now;
        self.last_click_pos = Some(position);

        // Determine selection type based on click count and modifiers
        let selection_type = if modifiers.alt_key() {
            SelectionType::Block
        } else {
            match self.click_count {
                2 => SelectionType::Semantic,
                3 => SelectionType::Lines,
                _ => SelectionType::Simple,
            }
        };

        let selection = Selection::new(position, selection_type);
        self.selection = Some(selection.clone());
        Some(selection)
    }

    /// Handle mouse movement (drag)
    pub fn handle_move(&mut self, position: Point) -> Option<Selection> {
        if let Some(ref mut selection) = self.selection {
            selection.update(position);
            return Some(selection.clone());
        }

        None
    }

    /// Finish the selection the primary button was making.
    pub fn handle_release(&mut self) {
        if let Some(ref mut selection) = self.selection {
            selection.finish();
            // Clear trivial (zero-length) selections so they don't
            // consume Escape keypresses intended for the terminal/PTY.
            // A click without drag creates a selection where start == end,
            // which would otherwise persist and cause ClearSelection to
            // swallow every Escape press.
            // Explicit word/line/block selection can intentionally be one
            // cell; only a plain click without a drag is trivial.
            if selection.is_empty() && selection.ty == SelectionType::Simple {
                self.selection = None;
            }
        }
    }

    /// Clear current selection
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Get current selection
    pub fn current_selection(&self) -> Option<&Selection> {
        self.selection.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_double_click_is_two_presses_on_one_target_within_the_interval() {
        let start = Instant::now();
        let mut clicks = DoubleClick::default();
        assert!(!clicks.press("a", start));
        assert!(clicks.press("a", start + DOUBLE_CLICK_INTERVAL));
        // The pair is spent; another press starts over.
        assert!(!clicks.press("a", start + DOUBLE_CLICK_INTERVAL));
        let late = start + DOUBLE_CLICK_INTERVAL * 2 + Duration::from_millis(1);
        assert!(!clicks.press("a", late));
        assert!(!clicks.press("b", late));
        clicks.reset();
        assert!(!clicks.press("b", late));
    }
}
