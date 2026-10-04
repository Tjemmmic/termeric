//! Rectangular-area operations: copy, fill, erase, attribute changes, and
//! checksums.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalRect {
    top: usize,
    bottom: usize,
    left: usize,
    right: usize,
}

impl TerminalRect {
    fn width(self) -> usize {
        self.right - self.left
    }

    fn height(self) -> usize {
        self.bottom - self.top
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DecAttributeChange {
    set: CellAttrs,
    clear: CellAttrs,
}

impl DecAttributeChange {
    fn set(&mut self, attrs: CellAttrs) {
        self.clear.remove(attrs);
        self.set.insert(attrs);
    }

    fn clear(&mut self, attrs: CellAttrs) {
        self.set.remove(attrs);
        self.clear.insert(attrs);
    }

    fn apply(self, attrs: &mut CellAttrs) {
        attrs.remove(self.clear);
        attrs.insert(self.set);
    }

    fn is_empty(self) -> bool {
        self.set.is_empty() && self.clear.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DecAttributeReverse {
    bold: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    hidden: bool,
}

impl DecAttributeReverse {
    fn is_empty(self) -> bool {
        !(self.bold || self.underline || self.blink || self.inverse || self.hidden)
    }

    fn apply(self, attrs: &mut CellAttrs) {
        if self.bold {
            attrs.toggle(CellAttrs::BOLD);
        }
        if self.underline {
            if attrs.intersects(CellAttrs::ALL_UNDERLINES) {
                attrs.remove(CellAttrs::ALL_UNDERLINES);
            } else {
                attrs.insert(CellAttrs::UNDERLINE);
            }
        }
        if self.blink {
            attrs.toggle(CellAttrs::BLINK);
        }
        if self.inverse {
            attrs.toggle(CellAttrs::INVERSE);
        }
        if self.hidden {
            attrs.toggle(CellAttrs::HIDDEN);
        }
    }
}

fn dec_checksum_attributes(cell: &Cell) -> u16 {
    let mut value = 0_u16;
    if cell.flags.contains(CellFlags::PROTECTED) {
        value |= 0x04;
    }
    if cell.attrs.contains(CellAttrs::HIDDEN) {
        value |= 0x08;
    }
    if cell.attrs.has_underline() {
        value |= 0x10;
    }
    if cell.attrs.contains(CellAttrs::INVERSE) {
        value |= 0x20;
    }
    if cell.attrs.contains(CellAttrs::BLINK) {
        value |= 0x40;
    }
    if cell.attrs.contains(CellAttrs::BOLD) {
        value |= 0x80;
    }
    value
}

fn dec_checksum_character(cell: &Cell, character: char) -> u16 {
    let dec = if cell.flags.contains(CellFlags::DEC_SPECIAL_GRAPHICS) {
        match character {
            ' ' => b'_',
            '◆' => b'`',
            '▒' => b'a',
            '␉' => b'b',
            '␌' => b'c',
            '␍' => b'd',
            '␊' => b'e',
            '°' => b'f',
            '±' => b'g',
            '␤' => b'h',
            '␋' => b'i',
            '┘' => b'j',
            '┐' => b'k',
            '┌' => b'l',
            '└' => b'm',
            '┼' => b'n',
            '⎺' => b'o',
            '⎻' => b'p',
            '─' => b'q',
            '⎼' => b'r',
            '⎽' => b's',
            '├' => b't',
            '┤' => b'u',
            '┴' => b'v',
            '┬' => b'w',
            '│' => b'x',
            '≤' => b'y',
            '≥' => b'z',
            'π' => b'{',
            '≠' => b'|',
            '£' => b'}',
            '·' => b'~',
            _ => (character as u32 & 0xff) as u8,
        }
    } else if cell.flags.contains(CellFlags::DEC_UK_CHARSET) && character == '£' {
        b'#'
    } else if character == '␦' {
        0x1b
    } else {
        (character as u32 & 0xff) as u8
    };
    u16::from(dec)
}

impl Terminal {
    /// Resolve VT's 1-indexed inclusive rectangle coordinates into an
    /// end-exclusive active-page rectangle. With DECOM set, coordinates are
    /// relative to and clipped by both margin pairs; otherwise margins do not
    /// affect rectangular operations.
    fn rectangular_area(
        &self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) -> Option<TerminalRect> {
        let origin = self.mode.contains(TerminalMode::ORIGIN);
        let (row_offset, row_limit) = if origin {
            (self.scroll_top, self.scroll_bottom)
        } else {
            (0, self.rows)
        };
        let (col_offset, col_limit) = if origin {
            (self.scroll_left, self.scroll_right)
        } else {
            (0, self.cols)
        };
        if row_offset >= row_limit || col_offset >= col_limit {
            return None;
        }

        let top = row_offset.saturating_add(top.max(1) - 1).min(row_limit - 1);
        let left = col_offset
            .saturating_add(left.max(1) - 1)
            .min(col_limit - 1);
        let bottom = if bottom == 0 {
            row_limit - 1
        } else {
            row_offset.saturating_add(bottom - 1).min(row_limit - 1)
        };
        let right = if right == 0 {
            col_limit - 1
        } else {
            col_offset.saturating_add(right - 1).min(col_limit - 1)
        };
        (top <= bottom && left <= right).then_some(TerminalRect {
            top,
            bottom: bottom + 1,
            left,
            right: right + 1,
        })
    }

    /// Permit or deny DECRQCRA screen checksum replies. This host security
    /// policy is intentionally independent of DECSTR and RIS terminal state.
    pub fn set_screen_checksum_reporting(&mut self, allowed: bool) {
        self.screen_checksum_reporting = allowed;
    }

    pub fn screen_checksum_reporting(&self) -> bool {
        self.screen_checksum_reporting
    }

    /// DECRQCRA — report the VT420 checksum for a rectangular area. Termeric
    /// has one page per screen buffer, so the requested page is normalized to
    /// the active page. Invalid/reversed rectangles return the DEC zero result.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn request_checksum_rectangular_area(
        &mut self,
        request_id: u16,
        _page: usize,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        if !self.screen_checksum_reporting {
            return;
        }
        let checksum = self
            .rectangular_area(top, left, bottom, right)
            .map_or(0, |area| self.checksum_area(area));
        self.push_event(TerminalEvent::PtyWrite(
            format!("\x1bP{request_id}!~{checksum:04X}\x1b\\").into_bytes(),
        ));
    }

    /// Match the default DEC/xterm algorithm: skip cells that have never held
    /// a drawn character, omit ordinary blanks after the first drawn cell,
    /// include the legacy presentation bits, subtract the 8-bit cell values,
    /// and wrap the result to 16 bits. Modern colors and combining scalars are
    /// deliberately outside the VT420 checksum.
    fn checksum_area(&self, area: TerminalRect) -> u16 {
        let mut total = 0_u16;
        let mut first_drawn = true;
        for row in area.top..area.bottom {
            for col in area.left..area.right {
                let Some(cell) = self.grid().cell(row, col) else {
                    continue;
                };
                let Some(character) = cell.content.char() else {
                    continue;
                };
                let attributes = dec_checksum_attributes(cell);
                let value = dec_checksum_character(cell, character).wrapping_add(attributes);
                if first_drawn || value != b' ' as u16 || attributes != 0 {
                    total = total.wrapping_add(value);
                }
                first_drawn = false;
            }
        }
        0_u16.wrapping_sub(total)
    }

    /// DECCRA — copy an overlap-safe rectangular cell snapshot. Termeric has
    /// one page per screen buffer, so every source/destination page value is
    /// clamped to that sole page as DEC specifies for unavailable page numbers.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy_rectangular_area(
        &mut self,
        source_top: usize,
        source_left: usize,
        source_bottom: usize,
        source_right: usize,
        _source_page: usize,
        destination_top: usize,
        destination_left: usize,
        _destination_page: usize,
    ) {
        let Some(source) =
            self.rectangular_area(source_top, source_left, source_bottom, source_right)
        else {
            return;
        };
        let destination_top = destination_top.max(1);
        let destination_left = destination_left.max(1);
        let destination_bottom = destination_top.saturating_add(source.height() - 1);
        let destination_right = destination_left.saturating_add(source.width() - 1);
        let Some(destination) = self.rectangular_area(
            destination_top,
            destination_left,
            destination_bottom,
            destination_right,
        ) else {
            return;
        };
        let height = source.height().min(destination.height());
        let width = source.width().min(destination.width());
        if height == 0
            || width == 0
            || (source.top == destination.top
                && source.left == destination.left
                && height == source.height()
                && width == source.width())
        {
            return;
        }

        let snapshot: Vec<Vec<Cell>> = (0..height)
            .map(|offset| {
                self.grid()
                    .row(source.top + offset)
                    .map(|row| row.iter().skip(source.left).take(width).cloned().collect())
                    .unwrap_or_default()
            })
            .collect();
        let blank = self.erase_cell();
        {
            let (grid, mut damage) = self.grid_with_selection_damage();
            grid.erase_multicells_with_damage(
                destination.top..destination.top + height,
                destination.left..destination.left + width,
                &blank,
                false,
                &mut damage,
            );
            for (offset, cells) in snapshot.iter().enumerate() {
                let row_index = destination.top + offset;
                if let Some(row) = grid.row_mut(row_index) {
                    row.replace_range(destination.left, cells, &blank, |cols| {
                        damage(row_index..row_index + 1, cols)
                    });
                }
            }
            grid.repair_multicells_with_damage(&blank, damage);
        }
        self.graphics.copy_rect(
            self.alt_screen_active,
            source.top..source.top + height,
            source.left..source.left + width,
            destination.top,
            destination.left,
        );
    }

    /// DECFRA — fill a rectangle with a printable GL/GR character using the
    /// current SGR and DECSCA attributes.
    pub(crate) fn fill_rectangular_area(
        &mut self,
        character: u16,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        let character = if character == 0 { 32 } else { character };
        if !((32..=126).contains(&character) || (160..=255).contains(&character)) {
            return;
        }
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        let mut character = char::from_u32(u32::from(character)).unwrap_or(' ');
        let charset = if character.is_ascii() {
            self.active_character_set()
        } else {
            CharacterSet::Ascii
        };
        character = charset.translate(character);
        let mut flags = CellFlags::empty();
        flags.set(CellFlags::PROTECTED, self.current_character_protected);
        flags.set(
            CellFlags::DEC_SPECIAL_GRAPHICS,
            charset == CharacterSet::DecSpecialGraphics,
        );
        flags.set(
            CellFlags::DEC_UK_CHARSET,
            charset == CharacterSet::UnitedKingdom,
        );
        let fill = Cell {
            content: CellContent::Char(character),
            fg: self.current_fg,
            bg: self.current_bg,
            fg_spec: self.current_fg_spec,
            bg_spec: self.current_bg_spec,
            underline_color: self.current_underline_color,
            underline_color_spec: self.current_underline_color_spec,
            attrs: self.current_attrs,
            flags,
            hyperlink_id: None,
            multicell: Default::default(),
        };
        let blank = self.erase_cell();
        {
            let (grid, mut damage) = self.grid_with_selection_damage();
            grid.erase_multicells_with_damage(
                area.top..area.bottom,
                area.left..area.right,
                &blank,
                false,
                &mut damage,
            );
            for row_index in area.top..area.bottom {
                if let Some(row) = grid.row_mut(row_index) {
                    row.fill_range(area.left, area.right, &fill, &blank, |cols| {
                        damage(row_index..row_index + 1, cols)
                    });
                }
            }
        }
        self.graphics.erase_rect(
            self.alt_screen_active,
            area.top..area.bottom,
            area.left..area.right,
        );
    }

    /// DECERA — replace every cell in a rectangle with a BCE blank, ignoring
    /// DECSCA protection and preserving the active cursor position.
    pub(crate) fn erase_rectangular_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.erase_cells_in_region(area.top..area.bottom, area.left..area.right, false);
        self.graphics.erase_rect(
            self.alt_screen_active,
            area.top..area.bottom,
            area.left..area.right,
        );
    }

    /// DECSERA — erase only unprotected grapheme content while retaining each
    /// affected cell's visual and protection attributes.
    pub(crate) fn selective_erase_rectangular_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
    ) {
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.erase_cells_in_region(area.top..area.bottom, area.left..area.right, true);
    }

    /// DECSACE — select whether DECCARA/DECRARA treat their endpoints as a
    /// wrapped stream or as an exact rectangle. Invalid values retain the
    /// existing setting.
    pub(crate) fn select_attribute_change_extent(&mut self, parameter: u16) {
        match parameter {
            0 | 1 => self.rectangular_change_extent = false,
            2 => self.rectangular_change_extent = true,
            _ => {}
        }
    }

    pub(crate) fn attribute_change_extent_parameter(&self) -> u8 {
        if self.rectangular_change_extent { 2 } else { 1 }
    }

    /// DECCARA — set or clear the DEC rendition subset in the selected area.
    /// Parameter order is significant, matching SGR-style last-change-wins
    /// behavior. SGR 8/28 are retained as the established xterm extension.
    pub(crate) fn change_attributes_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
        parameters: &[u16],
    ) {
        let mut change = DecAttributeChange::default();
        for parameter in parameters {
            match parameter {
                0 => change.clear(
                    CellAttrs::BOLD
                        | CellAttrs::ALL_UNDERLINES
                        | CellAttrs::BLINK
                        | CellAttrs::INVERSE,
                ),
                1 => change.set(CellAttrs::BOLD),
                4 => {
                    change.clear(CellAttrs::ALL_UNDERLINES);
                    change.set(CellAttrs::UNDERLINE);
                }
                5 => change.set(CellAttrs::BLINK),
                7 => change.set(CellAttrs::INVERSE),
                8 => change.set(CellAttrs::HIDDEN),
                22 => change.clear(CellAttrs::BOLD),
                24 => change.clear(CellAttrs::ALL_UNDERLINES),
                25 => change.clear(CellAttrs::BLINK),
                27 => change.clear(CellAttrs::INVERSE),
                28 => change.clear(CellAttrs::HIDDEN),
                _ => {}
            }
        }
        if change.is_empty() {
            return;
        }
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.apply_attribute_area(area, |attrs| change.apply(attrs));
    }

    /// DECRARA — reverse the requested DEC rendition subset. Repeated
    /// parameters cancel one another, and parameter 0 reverses the four DEC
    /// base attributes without implicitly toggling xterm's hidden extension.
    pub(crate) fn reverse_attributes_area(
        &mut self,
        top: usize,
        left: usize,
        bottom: usize,
        right: usize,
        parameters: &[u16],
    ) {
        let mut reverse = DecAttributeReverse::default();
        for parameter in parameters {
            match parameter {
                0 => {
                    reverse.bold ^= true;
                    reverse.underline ^= true;
                    reverse.blink ^= true;
                    reverse.inverse ^= true;
                }
                1 => reverse.bold ^= true,
                4 => reverse.underline ^= true,
                5 => reverse.blink ^= true,
                7 => reverse.inverse ^= true,
                8 => reverse.hidden ^= true,
                _ => {}
            }
        }
        if reverse.is_empty() {
            return;
        }
        let Some(area) = self.rectangular_area(top, left, bottom, right) else {
            return;
        };
        self.apply_attribute_area(area, |attrs| reverse.apply(attrs));
    }

    fn apply_attribute_area(&mut self, area: TerminalRect, mut apply: impl FnMut(&mut CellAttrs)) {
        let rectangular = self.rectangular_change_extent;
        let cols = self.cols;
        for row_index in area.top..area.bottom {
            let (start, end) = if rectangular || area.height() == 1 {
                (area.left, area.right)
            } else if row_index == area.top {
                (area.left, cols)
            } else if row_index + 1 == area.bottom {
                (0, area.right)
            } else {
                (0, cols)
            };
            let Some(row) = self.grid_mut().row_mut(row_index) else {
                continue;
            };
            row.change_attributes_range(start, end, rectangular, &mut apply);
        }
    }
}
