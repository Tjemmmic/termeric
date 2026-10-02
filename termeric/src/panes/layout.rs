//! Layout calculations for panes
//!
//! Handles the geometry of pane rectangles within the terminal window.

/// A rectangle describing a pane's position and size
#[derive(Debug, Clone, Copy)]
pub struct PaneRect {
    /// X position (left edge)
    pub x: f32,
    /// Y position (top edge)
    pub y: f32,
    /// Width in pixels
    pub width: f32,
    /// Height in pixels
    pub height: f32,
}

impl PaneRect {
    /// Create a new rectangle
    #[cfg(test)]
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Get the right edge
    pub fn right(&self) -> f32 {
        self.x + self.width
    }

    /// Get the bottom edge
    pub fn bottom(&self) -> f32 {
        self.y + self.height
    }

    /// Get the center X
    pub fn center_x(&self) -> f32 {
        self.x + self.width / 2.0
    }

    /// Get the center Y
    pub fn center_y(&self) -> f32 {
        self.y + self.height / 2.0
    }
}
