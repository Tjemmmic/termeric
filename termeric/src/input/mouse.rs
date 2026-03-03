//! Mouse input handling and coordinate conversion

use winit::event::MouseButton;
use winit::keyboard::ModifiersState;
use termeric_core::{Selection, SelectionType, Point};

/// Mouse input state tracker
pub struct MouseState {
    /// Current selection being made
    pub selection: Option<Selection>,
    /// Last mouse position in grid coordinates
    pub last_position: Option<Point>,
    /// Number of rapid clicks (for double/triple click detection)
    pub click_count: u32,
    /// Time of last click (for double/triple click detection)
    pub last_click_time: std::time::Instant,
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
            last_position: None,
            click_count: 0,
            last_click_time: std::time::Instant::now(),
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

        // Check for multi-click (double/triple)
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_click_time);
        let same_position = self.last_click_pos.map(|p| {
            (p.row - position.row).abs() <= 1 &&
            (p.col as i32 - position.col as i32).abs() <= 2
        }).unwrap_or(false);

        if elapsed.as_millis() < 400 && same_position {
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
        self.last_position = Some(position);

        if let Some(ref mut selection) = self.selection {
            selection.update(position);
            return Some(selection.clone());
        }

        None
    }

    /// Handle mouse button release
    pub fn handle_release(&mut self, button: MouseButton) -> Option<Selection> {
        if button != MouseButton::Left {
            return None;
        }

        if let Some(ref mut selection) = self.selection {
            selection.finish();
            let result = selection.clone();
            // Clear trivial (zero-length) selections so they don't
            // consume Escape keypresses intended for the terminal/PTY.
            // A click without drag creates a selection where start == end,
            // which would otherwise persist and cause ClearSelection to
            // swallow every Escape press.
            if selection.is_empty() {
                self.selection = None;
            }
            return Some(result);
        }

        None
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

/// Convert screen coordinates to grid cell position
/// tab_bar_height: height of the tab bar in pixels (subtracted from y before conversion)
pub fn screen_to_grid(
    x: f64,
    y: f64,
    cell_width: f32,
    cell_height: f32,
    tab_bar_height: f32,
    padding: f32,
    scroll_offset: i32,
) -> Point {
    let adjusted_x = (x as f32 - padding).max(0.0);
    let col = (adjusted_x / cell_width).floor() as usize;
    // Subtract tab bar height from y to get position relative to terminal area
    let adjusted_y = (y as f32 - tab_bar_height - padding).max(0.0);
    let row = (adjusted_y / cell_height).floor() as i32 - scroll_offset;
    Point::new(row, col)
}
