//! The frame the right-edge side panels (workspaces, Commands, Agents) share,
//! so they line up and their headers sit at the same place.

use crate::ui::geometry::Bounds;

/// The panel rectangle, its padding, and its header row with the close button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidePanelFrame {
    /// Slides in from the right edge and spans from `top` to the window's bottom.
    pub panel: Bounds,
    /// Horizontal padding between the panel's edges and its content.
    pub padding: f32,
    pub title_y: f32,
    /// The side of a square header control such as close.
    pub control: f32,
    pub close_button: Bounds,
}

impl SidePanelFrame {
    /// `top` is the bottom of the toolbar; `animation` runs from 0.0 (off the
    /// right edge) to 1.0 (fully in).
    pub fn calculate(
        window_width: f32,
        window_height: f32,
        top: f32,
        cell_width: f32,
        cell_height: f32,
        animation: f32,
    ) -> Self {
        // Wide enough that a hovered workspace row's actions leave room for its name.
        let width = (cell_width * 44.0).max(380.0).min(window_width * 0.45);
        let panel = Bounds {
            x: window_width - width * animation,
            y: top,
            width,
            height: (window_height - top).max(0.0),
        };
        let padding = cell_width * 1.5;
        let title_y = top + cell_height * 0.9;
        let control = cell_height * 1.5;
        Self {
            panel,
            padding,
            title_y,
            control,
            close_button: Bounds {
                x: panel.right() - padding * 0.5 - control,
                y: title_y,
                width: control,
                height: control,
            },
        }
    }
}
