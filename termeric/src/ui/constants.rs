//! Visual constants shared by the renderer and UI layout. Measurements are
//! in pixels unless noted otherwise.

/// Corner radius for overlay cards and modal panels.
pub const CARD_CORNER_RADIUS: f32 = 12.0;

/// Corner radius for small interactive buttons.
pub const BUTTON_CORNER_RADIUS: f32 = 6.0;

/// Standard border width for UI elements.
pub const BORDER_WIDTH: f32 = 1.0;

/// Filter fields with a leading magnifier: the icon's centre and the start
/// of the query text, in terminal cell widths from the field's left edge.
pub const FILTER_ICON_CENTER_CELLS: f32 = 1.3;
pub const FILTER_TEXT_INSET_CELLS: f32 = 2.6;

/// Character scale factor for content text in overlays (relative to cell_width).
pub const CONTENT_CHAR_SCALE: f32 = 0.85;
