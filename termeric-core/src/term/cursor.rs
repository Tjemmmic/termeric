//! Cursor state management

use crate::grid::{CellAttrs, Color, ColorSpec};

/// Saved cursor state (for DECSC/DECRC)
#[derive(Clone, Debug)]
pub struct SavedCursor {
    /// Saved row position
    pub row: usize,
    /// Saved column position
    pub col: usize,
    /// Saved text attributes
    pub attrs: CellAttrs,
    /// Saved foreground color (resolved RGB)
    pub fg: Color,
    /// Saved background color (resolved RGB)
    pub bg: Color,
    /// Saved foreground color specification
    pub fg_spec: ColorSpec,
    /// Saved background color specification
    pub bg_spec: ColorSpec,
}

impl Default for SavedCursor {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            attrs: CellAttrs::empty(),
            fg: Color::default_fg(),
            bg: Color::default_bg(),
            fg_spec: ColorSpec::Default,
            bg_spec: ColorSpec::Default,
        }
    }
}

/// Cursor shape/style
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    /// Block cursor (filled rectangle)
    #[default]
    Block,
    /// Underline cursor
    Underline,
    /// Bar/beam cursor (vertical line)
    Bar,
}

/// Cursor rendering state
#[derive(Clone, Debug)]
pub struct CursorState {
    /// Current style
    pub style: CursorStyle,
    /// Whether the cursor is blinking
    pub blinking: bool,
    /// Blink interval in milliseconds
    pub blink_interval_ms: u32,
    /// Style when window is not focused
    pub unfocused_style: UnfocusedCursorStyle,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            style: CursorStyle::Block,
            blinking: true,
            blink_interval_ms: 530,
            unfocused_style: UnfocusedCursorStyle::Hollow,
        }
    }
}

/// Cursor style when window is not focused
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnfocusedCursorStyle {
    /// Hollow block (outline only)
    #[default]
    Hollow,
    /// Same as focused style
    Same,
    /// Don't show cursor when unfocused
    None,
}
