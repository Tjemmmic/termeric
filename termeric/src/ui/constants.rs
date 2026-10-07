//! Visual constants shared by the renderer and UI layout. Chrome lengths are
//! logical pixels, scaled to the display where they are drawn
//! (`Renderer::px`); the rest are noted.

/// Corner radius for cards: toasts, the info overlay, the guardrail
/// confirmation, and Harvest panels.
pub const CARD_CORNER_RADIUS: f32 = 12.0;

/// Corner radius for the shared modal frame (Settings, the command palette).
pub const MODAL_CORNER_RADIUS: f32 = 10.0;

/// Corner radius for popup menus, dropdowns, and the find box.
pub const MENU_CORNER_RADIUS: f32 = 8.0;

/// Corner radius for buttons, tabs, and highlighted list rows.
pub const BUTTON_CORNER_RADIUS: f32 = 6.0;

/// Standard border width for UI elements.
pub const BORDER_WIDTH: f32 = 1.0;

/// The shortest a scrollbar's thumb gets, however long the list it scrolls.
pub const MIN_SCROLL_THUMB_HEIGHT: f32 = 24.0;

/// Filter fields with a leading magnifier: the icon's centre and the start
/// of the query text, in terminal cell widths from the field's left edge.
pub const FILTER_ICON_CENTER_CELLS: f32 = 1.3;
pub const FILTER_TEXT_INSET_CELLS: f32 = 2.6;

/// Character scale factor for content text in overlays (relative to cell_width).
pub const CONTENT_CHAR_SCALE: f32 = 0.85;
