//! Terminal cell definition
//!
//! Each cell in the terminal grid contains a character, colors, and attributes.

use bitflags::bitflags;

/// Alignment of fractionally scaled content inside an OSC 66 multicell block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum MultiCellAlignment {
    /// Align with the top or left edge.
    #[default]
    Start = 0,
    /// Align with the bottom or right edge.
    End = 1,
    /// Center within the occupied cell rectangle.
    Center = 2,
}

/// Compact, self-validating geometry for one cell in an OSC 66 text block.
///
/// The same scale, logical width, fraction, and alignment are stored in every
/// occupied cell. Only the top-left cell owns text; `relative_col` and
/// `relative_row` let any covered cell locate that leader without a heap
/// allocation or a separate spatial index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct MultiCell(u32);

impl MultiCell {
    const PRESENT: u32 = 1 << 31;
    const RELATIVE_COL_SHIFT: u32 = 0;
    const RELATIVE_ROW_SHIFT: u32 = 6;
    const SCALE_SHIFT: u32 = 9;
    const WIDTH_SHIFT: u32 = 12;
    const NUMERATOR_SHIFT: u32 = 15;
    const DENOMINATOR_SHIFT: u32 = 19;
    const HORIZONTAL_SHIFT: u32 = 23;
    const VERTICAL_SHIFT: u32 = 25;

    const RELATIVE_COL_MASK: u32 = 0x3f;
    const RELATIVE_ROW_MASK: u32 = 0x07;
    const THREE_BIT_MASK: u32 = 0x07;
    const FOUR_BIT_MASK: u32 = 0x0f;
    const ALIGNMENT_MASK: u32 = 0x03;
    const POSITION_MASK: u32 =
        Self::RELATIVE_COL_MASK | (Self::RELATIVE_ROW_MASK << Self::RELATIVE_ROW_SHIFT);

    /// Construct one occupied-cell descriptor. Returns `None` if any value is
    /// outside Kitty's protocol range or the relative position is not inside
    /// the resulting `scale * width` by `scale` block.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scale: u8,
        width: u8,
        numerator: u8,
        denominator: u8,
        horizontal: MultiCellAlignment,
        vertical: MultiCellAlignment,
        relative_col: u8,
        relative_row: u8,
    ) -> Option<Self> {
        if !(1..=7).contains(&scale)
            || !(1..=7).contains(&width)
            || numerator > 15
            || denominator > 15
            || (denominator == 0 && numerator != 0)
            || (denominator != 0 && numerator >= denominator)
            || relative_col >= scale.saturating_mul(width)
            || relative_row >= scale
        {
            return None;
        }
        let value = Self::PRESENT
            | u32::from(relative_col) << Self::RELATIVE_COL_SHIFT
            | u32::from(relative_row) << Self::RELATIVE_ROW_SHIFT
            | u32::from(scale) << Self::SCALE_SHIFT
            | u32::from(width) << Self::WIDTH_SHIFT
            | u32::from(numerator) << Self::NUMERATOR_SHIFT
            | u32::from(denominator) << Self::DENOMINATOR_SHIFT
            | (horizontal as u32) << Self::HORIZONTAL_SHIFT
            | (vertical as u32) << Self::VERTICAL_SHIFT;
        Some(Self(value))
    }

    /// Whether this descriptor represents a multicell block.
    pub fn is_present(self) -> bool {
        self.0 & Self::PRESENT != 0
    }

    /// Overall integer scale (`1..=7`).
    pub fn scale(self) -> u8 {
        ((self.0 >> Self::SCALE_SHIFT) & Self::THREE_BIT_MASK) as u8
    }

    /// Logical width before multiplying by `scale` (`1..=7`).
    pub(crate) fn logical_width(self) -> u8 {
        ((self.0 >> Self::WIDTH_SHIFT) & Self::THREE_BIT_MASK) as u8
    }

    /// Occupied columns in the terminal grid.
    pub fn columns(self) -> usize {
        usize::from(self.scale()) * usize::from(self.logical_width())
    }

    /// Occupied rows in the terminal grid.
    pub fn rows(self) -> usize {
        usize::from(self.scale())
    }

    /// Fractional-scale numerator, or zero when fractional scaling is off.
    pub fn numerator(self) -> u8 {
        ((self.0 >> Self::NUMERATOR_SHIFT) & Self::FOUR_BIT_MASK) as u8
    }

    /// Fractional-scale denominator, or zero when fractional scaling is off.
    pub fn denominator(self) -> u8 {
        ((self.0 >> Self::DENOMINATOR_SHIFT) & Self::FOUR_BIT_MASK) as u8
    }

    /// Effective font-size multiplier relative to the base font.
    pub fn font_scale(self) -> f32 {
        let scale = f32::from(self.scale());
        if self.denominator() == 0 {
            scale
        } else {
            scale * f32::from(self.numerator()) / f32::from(self.denominator())
        }
    }

    /// Horizontal alignment inside the occupied rectangle.
    pub fn horizontal_alignment(self) -> MultiCellAlignment {
        alignment_from_bits((self.0 >> Self::HORIZONTAL_SHIFT) & Self::ALIGNMENT_MASK)
    }

    /// Vertical alignment inside the occupied rectangle.
    pub fn vertical_alignment(self) -> MultiCellAlignment {
        alignment_from_bits((self.0 >> Self::VERTICAL_SHIFT) & Self::ALIGNMENT_MASK)
    }

    /// Column offset from this cell back to the block leader.
    pub(crate) fn relative_col(self) -> usize {
        ((self.0 >> Self::RELATIVE_COL_SHIFT) & Self::RELATIVE_COL_MASK) as usize
    }

    /// Row offset from this cell back to the block leader.
    pub(crate) fn relative_row(self) -> usize {
        ((self.0 >> Self::RELATIVE_ROW_SHIFT) & Self::RELATIVE_ROW_MASK) as usize
    }

    /// Whether this cell is the top-left text-owning leader.
    pub fn is_leader(self) -> bool {
        self.is_present() && self.relative_col() == 0 && self.relative_row() == 0
    }

    /// Return the descriptor for another position in the same block.
    pub(crate) fn at(self, relative_col: usize, relative_row: usize) -> Option<Self> {
        if !self.is_present()
            || relative_col >= self.columns()
            || relative_row >= self.rows()
            || relative_col > Self::RELATIVE_COL_MASK as usize
            || relative_row > Self::RELATIVE_ROW_MASK as usize
        {
            return None;
        }
        Some(Self(
            (self.0 & !Self::POSITION_MASK)
                | (relative_col as u32) << Self::RELATIVE_COL_SHIFT
                | (relative_row as u32) << Self::RELATIVE_ROW_SHIFT,
        ))
    }

    /// Whether two occupied cells describe the same layout independent of
    /// their relative positions.
    pub(crate) fn has_same_layout(self, other: Self) -> bool {
        self.is_present()
            && other.is_present()
            && (self.0 & !Self::POSITION_MASK) == (other.0 & !Self::POSITION_MASK)
    }

    #[cfg(test)]
    pub(crate) fn raw(self) -> u32 {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
}

fn alignment_from_bits(bits: u32) -> MultiCellAlignment {
    match bits {
        1 => MultiCellAlignment::End,
        2 => MultiCellAlignment::Center,
        _ => MultiCellAlignment::Start,
    }
}

/// RGB color
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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

    /// Default foreground color - cream/tan (turmeric skin)
    pub const fn default_fg() -> Self {
        Self::rgb(212, 196, 168) // #D4C4A8
    }

    /// Default background color - dark brown (dried turmeric)
    pub const fn default_bg() -> Self {
        Self::rgb(28, 20, 16) // #1C1410
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
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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
        /// Dotted underline
        const DOTTED_UNDERLINE = 0b1000_0000_0000;
        /// Dashed underline
        const DASHED_UNDERLINE = 0b0001_0000_0000_0000;
    }
}

impl CellAttrs {
    /// All underline-related flags
    pub(crate) const ALL_UNDERLINES: CellAttrs = CellAttrs::from_bits_truncate(
        CellAttrs::UNDERLINE.bits()
            | CellAttrs::DOUBLE_UNDERLINE.bits()
            | CellAttrs::CURLY_UNDERLINE.bits()
            | CellAttrs::DOTTED_UNDERLINE.bits()
            | CellAttrs::DASHED_UNDERLINE.bits(),
    );

    /// Check if any underline style is active
    pub fn has_underline(&self) -> bool {
        self.intersects(Self::ALL_UNDERLINES)
    }
}

bitflags! {
    /// Cell flags for special handling
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct CellFlags: u8 {
        /// This cell contains a wide character (takes 2 cells)
        const WIDE_CHAR = 0b0001;
        /// This cell is a spacer for the previous wide character
        const WIDE_CHAR_SPACER = 0b0010;
        /// This cell is part of a hyperlink
        const HYPERLINK = 0b0100;
        /// DEC character protection selected by DECSCA. Selective erase
        /// operations preserve cells carrying this flag.
        const PROTECTED = 0b1000;
        /// The displayed glyph originated in DEC Special Graphics. Retaining
        /// the G-set identity makes DECRQCRA reversible without another byte
        /// in every dense cell.
        const DEC_SPECIAL_GRAPHICS = 0b1_0000;
        /// The displayed glyph originated in the United Kingdom G-set.
        const DEC_UK_CHARSET = 0b10_0000;
    }
}

/// Content of a cell
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum CellContent {
    /// Empty cell
    #[default]
    Empty,
    /// Single character (common case, optimized)
    Char(char),
    /// Grapheme cluster (multiple codepoints)
    Grapheme(Box<str>),
}

impl CellContent {
    /// Check if the cell is empty
    pub fn is_empty(&self) -> bool {
        matches!(self, CellContent::Empty)
    }

    /// Get the character if this is a single char
    pub fn char(&self) -> Option<char> {
        match self {
            CellContent::Char(c) => Some(*c),
            CellContent::Grapheme(s) => s.chars().next(),
            CellContent::Empty => None,
        }
    }

    /// Append this cell's complete text content.
    pub fn push_to(&self, output: &mut String) {
        match self {
            Self::Empty => {}
            Self::Char(character) => output.push(*character),
            Self::Grapheme(grapheme) => output.push_str(grapheme),
        }
    }

    /// Return an owned representation suitable for grapheme extension.
    pub fn to_text(&self) -> String {
        match self {
            Self::Empty => String::new(),
            Self::Char(character) => character.to_string(),
            Self::Grapheme(grapheme) => grapheme.to_string(),
        }
    }
}

/// How a color was originally specified by the program.
///
/// Preserving this allows session restore to re-resolve indexed colors
/// through the current theme, so sessions adapt to theme changes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorSpec {
    /// Terminal default color (fg or bg)
    #[default]
    Default,
    /// ANSI palette color (0-7 normal, 8-15 bright, 16-255 extended)
    Indexed(u8),
    /// Explicit 24-bit RGB color
    TrueColor(Color),
}

/// A single cell in the terminal grid
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The character(s) displayed in this cell
    pub content: CellContent,
    /// Foreground color (resolved RGB for rendering)
    pub fg: Color,
    /// Background color (resolved RGB for rendering)
    pub bg: Color,
    /// How the foreground color was originally specified
    pub fg_spec: ColorSpec,
    /// How the background color was originally specified
    pub bg_spec: ColorSpec,
    /// Underline color (None means use fg color)
    pub underline_color: Option<Color>,
    /// How the underline color was originally specified. This is retained
    /// separately from the resolved color because Kitty Unicode placeholders
    /// encode placement IDs in the original color value.
    pub underline_color_spec: Option<ColorSpec>,
    /// Text attributes (bold, italic, etc.)
    pub attrs: CellAttrs,
    /// Cell flags (wide char, hyperlink, etc.)
    pub flags: CellFlags,
    /// Hyperlink ID (index into hyperlink registry, if this cell is part of a hyperlink)
    pub hyperlink_id: Option<u32>,
    /// Compact OSC 66 block geometry. Zero means ordinary terminal content.
    pub multicell: MultiCell,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            content: CellContent::Empty,
            fg: Color::default_fg(),
            bg: Color::default_bg(),
            fg_spec: ColorSpec::Default,
            bg_spec: ColorSpec::Default,
            underline_color: None,
            underline_color_spec: None,
            attrs: CellAttrs::empty(),
            flags: CellFlags::empty(),
            hyperlink_id: None,
            multicell: MultiCell::default(),
        }
    }
}

impl Cell {
    /// Whether this cell can be omitted from history or reflow padding without
    /// losing durable presentation state.
    pub(crate) fn is_history_default(&self) -> bool {
        self.content.is_empty()
            && self.fg == Color::default_fg()
            && self.bg == Color::default_bg()
            && self.fg_spec == ColorSpec::Default
            && self.bg_spec == ColorSpec::Default
            && self.underline_color.is_none()
            && self.underline_color_spec.is_none()
            && self.attrs.is_empty()
            && self.flags.is_empty()
            && self.hyperlink_id.is_none()
            && !self.multicell.is_present()
    }

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
    pub(crate) fn set_char(&mut self, c: char) {
        self.content = CellContent::Char(c);
        self.multicell = MultiCell::default();
    }

    /// Set the grapheme content (for complex Unicode)
    pub(crate) fn set_grapheme(&mut self, s: &str) {
        let mut scalars = s.chars();
        self.content = match (scalars.next(), scalars.next()) {
            (None, _) => CellContent::Empty,
            (Some(scalar), None) => CellContent::Char(scalar),
            (Some(_), Some(_)) => CellContent::Grapheme(s.into()),
        };
        self.multicell = MultiCell::default();
    }

    /// Clear the cell to default state
    pub fn clear(&mut self) {
        self.content = CellContent::Empty;
        self.fg = Color::default_fg();
        self.bg = Color::default_bg();
        self.fg_spec = ColorSpec::Default;
        self.bg_spec = ColorSpec::Default;
        self.underline_color = None;
        self.underline_color_spec = None;
        self.attrs = CellAttrs::empty();
        self.flags = CellFlags::empty();
        self.hyperlink_id = None;
        self.multicell = MultiCell::default();
    }

    /// Remove only the displayed grapheme and hyperlink identity while
    /// retaining visual and DEC protection attributes. DEC selective erase
    /// uses this distinction instead of creating a newly styled BCE blank.
    pub(crate) fn clear_content_preserving_attrs(&mut self) {
        self.content = CellContent::Empty;
        self.flags.remove(
            CellFlags::WIDE_CHAR
                | CellFlags::WIDE_CHAR_SPACER
                | CellFlags::HYPERLINK
                | CellFlags::DEC_SPECIAL_GRAPHICS
                | CellFlags::DEC_UK_CHARSET,
        );
        self.hyperlink_id = None;
        self.multicell = MultiCell::default();
    }

    /// Reset the cell completely (including colors)
    pub fn reset(&mut self) {
        *self = Self::default();
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
        if self.multicell.is_leader() {
            self.multicell.columns()
        } else if self.multicell.is_present() {
            0
        } else if self.is_wide_char() {
            2
        } else if self.is_wide_char_spacer() {
            0
        } else {
            1
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

    #[test]
    fn multicell_descriptor_round_trips_every_protocol_field() {
        let descriptor = MultiCell::new(
            7,
            7,
            14,
            15,
            MultiCellAlignment::Center,
            MultiCellAlignment::End,
            48,
            6,
        )
        .unwrap();
        assert!(descriptor.is_present());
        assert!(!descriptor.is_leader());
        assert_eq!(descriptor.scale(), 7);
        assert_eq!(descriptor.logical_width(), 7);
        assert_eq!(descriptor.columns(), 49);
        assert_eq!(descriptor.rows(), 7);
        assert_eq!(descriptor.numerator(), 14);
        assert_eq!(descriptor.denominator(), 15);
        assert_eq!(
            descriptor.horizontal_alignment(),
            MultiCellAlignment::Center
        );
        assert_eq!(descriptor.vertical_alignment(), MultiCellAlignment::End);
        assert_eq!(descriptor.relative_col(), 48);
        assert_eq!(descriptor.relative_row(), 6);
        assert_eq!(MultiCell::from_raw(descriptor.raw()), descriptor);

        let leader = MultiCell::new(
            2,
            3,
            0,
            0,
            MultiCellAlignment::Start,
            MultiCellAlignment::Start,
            0,
            0,
        )
        .unwrap();
        assert!(leader.is_leader());
        let mut cell = Cell::with_char('x');
        cell.multicell = leader;
        assert_eq!(cell.width(), 6);
        cell.clear();
        assert!(!cell.multicell.is_present());
    }

    #[test]
    fn multicell_descriptor_rejects_impossible_geometry() {
        assert!(std::mem::size_of::<MultiCell>() <= 4);
        assert!(std::mem::size_of::<Cell>() <= 64);
        let start = MultiCellAlignment::Start;
        assert!(MultiCell::new(0, 1, 0, 0, start, start, 0, 0).is_none());
        assert!(MultiCell::new(1, 0, 0, 0, start, start, 0, 0).is_none());
        assert!(MultiCell::new(1, 1, 1, 0, start, start, 0, 0).is_none());
        assert!(MultiCell::new(1, 1, 1, 1, start, start, 0, 0).is_none());
        assert!(MultiCell::new(2, 2, 0, 0, start, start, 4, 0).is_none());
        assert!(MultiCell::new(2, 2, 0, 0, start, start, 0, 2).is_none());
    }
}
