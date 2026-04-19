//! Layout calculations for panes
//!
//! Handles the geometry of pane rectangles within the terminal window.

use super::tree::PaneId;
use std::collections::HashMap;

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

    /// Check if a point is inside this rectangle
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    /// Inset the rectangle by a given amount
    pub fn inset(&self, amount: f32) -> Self {
        Self {
            x: self.x + amount,
            y: self.y + amount,
            width: (self.width - amount * 2.0).max(0.0),
            height: (self.height - amount * 2.0).max(0.0),
        }
    }
}

/// Layout information for all panes
#[derive(Debug, Clone)]
pub struct PaneLayout {
    /// Map from pane ID to rectangle
    rects: HashMap<PaneId, PaneRect>,
}

impl PaneLayout {
    /// Create a new empty layout
    pub fn new() -> Self {
        Self {
            rects: HashMap::new(),
        }
    }

    /// Insert a pane rectangle
    pub fn insert(&mut self, id: PaneId, rect: PaneRect) {
        self.rects.insert(id, rect);
    }

    /// Get a pane's rectangle
    pub fn get(&self, id: &PaneId) -> Option<&PaneRect> {
        self.rects.get(id)
    }

    /// Iterate over all panes
    pub fn iter(&self) -> impl Iterator<Item = (&PaneId, &PaneRect)> {
        self.rects.iter()
    }

    /// Find the pane at a given position
    pub fn pane_at(&self, x: f32, y: f32) -> Option<PaneId> {
        self.rects
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
            .map(|(&id, _)| id)
    }
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_contains() {
        let rect = PaneRect::new(10.0, 20.0, 100.0, 80.0);

        assert!(rect.contains(50.0, 50.0)); // Inside
        assert!(rect.contains(10.0, 20.0)); // Top-left corner
        assert!(!rect.contains(5.0, 50.0)); // Left of rect
        assert!(!rect.contains(120.0, 50.0)); // Right of rect
        assert!(!rect.contains(50.0, 10.0)); // Above rect
        assert!(!rect.contains(50.0, 110.0)); // Below rect
    }

    #[test]
    fn test_rect_inset() {
        let rect = PaneRect::new(0.0, 0.0, 100.0, 80.0);
        let inset = rect.inset(10.0);

        assert_eq!(inset.x, 10.0);
        assert_eq!(inset.y, 10.0);
        assert_eq!(inset.width, 80.0);
        assert_eq!(inset.height, 60.0);
    }

    #[test]
    fn test_layout_pane_at() {
        let mut layout = PaneLayout::new();
        layout.insert(PaneId(0), PaneRect::new(0.0, 0.0, 400.0, 300.0));
        layout.insert(PaneId(1), PaneRect::new(400.0, 0.0, 400.0, 300.0));

        assert_eq!(layout.pane_at(200.0, 150.0), Some(PaneId(0)));
        assert_eq!(layout.pane_at(600.0, 150.0), Some(PaneId(1)));
        assert_eq!(layout.pane_at(800.0, 150.0), None);
    }
}
