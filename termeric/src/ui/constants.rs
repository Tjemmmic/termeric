//! Shared UI visual constants
//!
//! Centralizes hardcoded visual values that were scattered across the renderer
//! and UI modules. All measurements are in pixels unless noted otherwise.

/// Corner radius for overlay cards and modal panels.
pub const CARD_CORNER_RADIUS: f32 = 12.0;

/// Corner radius for small interactive buttons.
pub const BUTTON_CORNER_RADIUS: f32 = 6.0;

/// Standard border width for UI elements.
pub const BORDER_WIDTH: f32 = 1.0;

/// Font line-height multiplier (relative to font size).
pub const LINE_HEIGHT_MULTIPLIER: f32 = 1.2;

/// Character scale factor for content text in overlays (relative to cell_width).
pub const CONTENT_CHAR_SCALE: f32 = 0.85;
