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

