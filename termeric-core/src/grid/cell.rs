//! Terminal cell definition
//!
//! Each cell in the terminal grid contains a character, colors, and attributes.

use bitflags::bitflags;

/// RGB color
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    /// Create a new RGB color
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Default foreground color (white)
    pub const fn default_fg() -> Self {
        Self::rgb(229, 229, 229)
    }

    /// Default background color (black)
    pub const fn default_bg() -> Self {
        Self::rgb(0, 0, 0)
    }

    /// Convert to floating point array for shaders
    pub fn to_f32_array(&self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            1.0,
        ]
    }
}

bitflags! {
    /// Cell text attributes (bold, italic, etc.)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CellAttrs: u16 {
        /// Bold/increased intensity
        const BOLD       = 0b0000_0000_0001;
        /// Dim/faint/decreased intensity
        const DIM        = 0b0000_0000_0010;
        /// Italic
        const ITALIC     = 0b0000_0000_0100;
        /// Underline
        const UNDERLINE  = 0b0000_0000_1000;
        /// Slow blink
        const BLINK      = 0b0000_0001_0000;
        /// Rapid blink (often same as slow blink)
        const RAPID_BLINK = 0b0000_0010_0000;
        /// Inverse/reverse video
        const INVERSE    = 0b0000_0100_0000;
        /// Hidden/invisible
        const HIDDEN     = 0b0000_1000_0000;
        /// Strikethrough
        const STRIKE     = 0b0001_0000_0000;
        /// Double underline
        const DOUBLE_UNDERLINE = 0b0010_0000_0000;
        /// Curly/wavy underline
        const CURLY_UNDERLINE = 0b0100_0000_0000;
    }
}

bitflags! {
    /// Cell flags for special handling
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct CellFlags: u8 {
        /// This cell contains a wide character (takes 2 cells)
        const WIDE_CHAR = 0b0001;
        /// This cell is a spacer for the previous wide character
        const WIDE_CHAR_SPACER = 0b0010;
        /// This cell is part of a hyperlink
        const HYPERLINK = 0b0100;
        /// This cell has been modified since last render
        const DIRTY = 0b1000;
    }
}

/// Content of a cell
#[derive(Clone, Debug)]
pub enum CellContent {
    /// Empty cell
    Empty,
    /// Single character (common case, optimized)
    Char(char),
    /// Grapheme cluster (multiple codepoints)
    Grapheme(Box<str>),
}

impl Default for CellContent {
    fn default() -> Self {
        Self::Empty
    }
}

impl CellContent {
    /// Check if the cell is empty
    pub fn is_empty(&self) -> bool {
        matches!(self, CellContent::Empty)
    }

    /// Get the content as a string slice
    pub fn as_str(&self) -> &str {
        match self {
            CellContent::Empty => "",
            CellContent::Char(c) => {
                // This is a bit of a hack, but chars are always valid UTF-8
                // We'd need a buffer to return a &str for a char
                // In practice, callers should handle Char separately
                ""
            }
            CellContent::Grapheme(s) => s,
        }
    }

    /// Get the character if this is a single char
    pub fn char(&self) -> Option<char> {
        match self {
            CellContent::Char(c) => Some(*c),
            CellContent::Grapheme(s) => s.chars().next(),
            CellContent::Empty => None,
        }
    }
}

/// A single cell in the terminal grid
#[derive(Clone, Debug)]
pub struct Cell {
    /// The character(s) displayed in this cell
    pub content: CellContent,
    /// Foreground color
    pub fg: Color,
    /// Background color
    pub bg: Color,
    /// Text attributes (bold, italic, etc.)
    pub attrs: CellAttrs,
    /// Cell flags (wide char, hyperlink, etc.)
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            content: CellContent::Empty,
            fg: Color::default_fg(),
            bg: Color::default_bg(),
            attrs: CellAttrs::empty(),
            flags: CellFlags::empty(),
        }
    }
}

impl Cell {
    /// Create a new empty cell
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a cell with a character
    pub fn with_char(c: char) -> Self {
        Self {
            content: CellContent::Char(c),
            ..Default::default()
        }
    }

    /// Set the character content
    pub fn set_char(&mut self, c: char) {
        self.content = CellContent::Char(c);
        self.flags.insert(CellFlags::DIRTY);
    }

    /// Set the grapheme content (for complex Unicode)
    pub fn set_grapheme(&mut self, s: &str) {
        self.content = CellContent::Grapheme(s.into());
        self.flags.insert(CellFlags::DIRTY);
    }

    /// Clear the cell to default state
    pub fn clear(&mut self) {
        self.content = CellContent::Empty;
        self.attrs = CellAttrs::empty();
        self.flags = CellFlags::DIRTY; // Keep dirty flag to trigger re-render
    }

    /// Reset the cell completely (including colors)
    pub fn reset(&mut self) {
        *self = Self::default();
        self.flags.insert(CellFlags::DIRTY);
    }

    /// Check if the cell is empty
    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Check if this cell is a wide character spacer
    pub fn is_wide_char_spacer(&self) -> bool {
        self.flags.contains(CellFlags::WIDE_CHAR_SPACER)
    }

    /// Check if this cell contains a wide character
    pub fn is_wide_char(&self) -> bool {
        self.flags.contains(CellFlags::WIDE_CHAR)
    }

    /// Get the display width of this cell (1 for normal, 2 for wide)
    pub fn width(&self) -> usize {
        if self.is_wide_char() {
            2
        } else if self.is_wide_char_spacer() {
            0
        } else {
            1
        }
    }

    /// Apply the inverse attribute (swap fg/bg)
    pub fn apply_inverse(&self) -> (Color, Color) {
        if self.attrs.contains(CellAttrs::INVERSE) {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_default() {
        let cell = Cell::default();
        assert!(cell.is_empty());
        assert_eq!(cell.width(), 1);
    }

    #[test]
    fn test_cell_with_char() {
        let cell = Cell::with_char('A');
        assert!(!cell.is_empty());
        assert_eq!(cell.content.char(), Some('A'));
    }

    #[test]
    fn test_cell_clear() {
        let mut cell = Cell::with_char('A');
        cell.clear();
        assert!(cell.is_empty());
    }

    #[test]
    fn test_color_conversion() {
        let color = Color::rgb(255, 128, 0);
        let arr = color.to_f32_array();
        assert!((arr[0] - 1.0).abs() < 0.01);
        assert!((arr[1] - 0.502).abs() < 0.01);
        assert!((arr[2] - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_cell_attrs() {
        let mut attrs = CellAttrs::empty();
        attrs.insert(CellAttrs::BOLD);
        attrs.insert(CellAttrs::ITALIC);
        assert!(attrs.contains(CellAttrs::BOLD));
        assert!(attrs.contains(CellAttrs::ITALIC));
        assert!(!attrs.contains(CellAttrs::UNDERLINE));
    }
}
