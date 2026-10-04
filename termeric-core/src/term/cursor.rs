//! Cursor state management

use crate::grid::{CellAttrs, Color, ColorSpec};

use super::CharacterSet;

/// Cursor shape selected by the child application with DECSCUSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalCursorShape {
    Block,
    Underline,
    Beam,
}

/// Application-owned cursor presentation override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursorStyle {
    pub shape: TerminalCursorShape,
    pub blinking: bool,
    parameter: u8,
}

impl TerminalCursorStyle {
    pub(super) fn from_parameter(parameter: u16) -> Option<Self> {
        let style = match parameter {
            1 => Self {
                shape: TerminalCursorShape::Block,
                blinking: true,
                parameter: 1,
            },
            2 => Self {
                shape: TerminalCursorShape::Block,
                blinking: false,
                parameter: 2,
            },
            3 => Self {
                shape: TerminalCursorShape::Underline,
                blinking: true,
                parameter: 3,
            },
            4 => Self {
                shape: TerminalCursorShape::Underline,
                blinking: false,
                parameter: 4,
            },
            5 => Self {
                shape: TerminalCursorShape::Beam,
                blinking: true,
                parameter: 5,
            },
            6 => Self {
                shape: TerminalCursorShape::Beam,
                blinking: false,
                parameter: 6,
            },
            _ => return None,
        };
        Some(style)
    }

    pub(super) fn parameter(self) -> u8 {
        self.parameter
    }
}

/// Saved cursor state (for DECSC/DECRC)
#[derive(Clone, Debug)]
pub(crate) struct SavedCursor {
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
    /// Saved underline color (None means follow foreground)
    pub underline_color: Option<Color>,
    /// Original saved underline-color encoding.
    pub underline_color_spec: Option<ColorSpec>,
    /// Saved G0 through G3 character-set designations.
    pub(super) g_sets: [CharacterSet; 4],
    /// Saved GL and GR invocation registers.
    pub(super) gl_register: u8,
    pub(super) gr_register: u8,
    /// Saved one-shot G2/G3 invocation.
    pub(super) single_shift: Option<u8>,
    /// Current DECSCA selective-erase protection for newly written cells.
    pub(super) character_protected: bool,
    /// Whether the next graphic character must perform delayed autowrap.
    pub(super) wrap_pending: bool,
    /// DEC origin mode at the time of DECSC.
    pub(super) origin_mode: bool,
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
            underline_color: None,
            underline_color_spec: None,
            g_sets: [CharacterSet::Ascii; 4],
            gl_register: 0,
            gr_register: 2,
            single_shift: None,
            character_protected: false,
            wrap_pending: false,
            origin_mode: false,
        }
    }
}
