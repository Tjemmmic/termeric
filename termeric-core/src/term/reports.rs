//! Mode, window, cursor, tab-stop, presentation-state, and setting reports.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CursorPresentationState {
    row: usize,
    col: usize,
    bold: bool,
    underline: bool,
    blink: bool,
    inverse: bool,
    protected: bool,
    wrap_pending: bool,
    origin_mode: bool,
    gl_register: u8,
    gr_register: u8,
    single_shift: Option<u8>,
    g_sets: [CharacterSet; 4],
}

impl CursorPresentationState {
    fn parse(data: &[u8], rows: usize, cols: usize) -> Option<Self> {
        // DECCIR has ten small, fixed-format fields. Bound this separately from
        // the much larger DCS allowance required by graphics payloads so a
        // malformed report cannot amplify into a large temporary field vector.
        if data.len() > 128 {
            return None;
        }
        let fields: Vec<&[u8]> = data.split(|byte| *byte == b';').collect();
        if fields.len() != 10 {
            return None;
        }
        let row = parse_decimal(fields[0])?;
        let col = parse_decimal(fields[1])?;
        if row == 0 || row > rows || col == 0 || col > cols || parse_decimal(fields[2])? != 1 {
            return None;
        }

        let rendition = report_byte(fields[3], 0x0f)?;
        let protection = report_byte(fields[4], 0x01)?;
        let flags = report_byte(fields[5], 0x0f)?;
        if flags & 0x06 == 0x06 {
            // SS2 and SS3 are mutually exclusive pending states.
            return None;
        }
        let gl_register = u8::try_from(parse_decimal(fields[6])?).ok()?;
        let gr_register = u8::try_from(parse_decimal(fields[7])?).ok()?;
        if gl_register >= 4 || gr_register >= 4 || fields[8] != b"@" {
            return None;
        }
        let [g0, g1, g2, g3] = fields[9] else {
            return None;
        };
        let g_sets = [
            CharacterSet::from_designator(*g0)?,
            CharacterSet::from_designator(*g1)?,
            CharacterSet::from_designator(*g2)?,
            CharacterSet::from_designator(*g3)?,
        ];

        Some(Self {
            row: row - 1,
            col: col - 1,
            bold: rendition & 0x01 != 0,
            underline: rendition & 0x02 != 0,
            blink: rendition & 0x04 != 0,
            inverse: rendition & 0x08 != 0,
            protected: protection & 0x01 != 0,
            wrap_pending: flags & 0x08 != 0,
            origin_mode: flags & 0x01 != 0,
            gl_register,
            gr_register,
            single_shift: if flags & 0x04 != 0 {
                Some(3)
            } else if flags & 0x02 != 0 {
                Some(2)
            } else {
                None
            },
            g_sets,
        })
    }
}

fn report_byte(field: &[u8], allowed_bits: u8) -> Option<u8> {
    let [value] = field else {
        return None;
    };
    (*value & !allowed_bits == b'@').then_some(*value)
}

fn parse_decimal(field: &[u8]) -> Option<usize> {
    if field.is_empty() {
        return None;
    }
    field.iter().try_fold(0_usize, |value, byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
        value.checked_mul(10)?.checked_add(usize::from(digit))
    })
}

fn parse_tab_stop_report(data: &[u8], cols: usize) -> Option<Vec<bool>> {
    let digits = cols.max(1).to_string().len();
    if data.len() > cols.saturating_mul(digits.saturating_add(1)) {
        return None;
    }
    let mut tabs = vec![false; cols];
    if data.is_empty() {
        return Some(tabs);
    }
    for (count, field) in data.split(|byte| *byte == b'/').enumerate() {
        if count >= cols {
            return None;
        }
        let column = parse_decimal(field)?;
        if column == 0 || column > cols {
            return None;
        }
        tabs[column - 1] = true;
    }
    Some(tabs)
}

impl Terminal {
    /// Return a DECRPM status value for a mode whose behavior is represented
    /// authoritatively by this terminal. Unknown or momentary modes report 0.
    pub(crate) fn mode_status(&self, private: bool, number: u16) -> u8 {
        let enabled = if private {
            match number {
                1 => Some(self.mode.contains(TerminalMode::CURSOR_KEYS_APPLICATION)),
                6 => Some(self.mode.contains(TerminalMode::ORIGIN)),
                7 => Some(self.mode.contains(TerminalMode::AUTOWRAP)),
                9 => Some(self.mode.contains(TerminalMode::MOUSE_X10)),
                12 => Some(self.mode.contains(TerminalMode::CURSOR_BLINKING)),
                25 => Some(self.mode.contains(TerminalMode::CURSOR_VISIBLE)),
                69 => Some(self.mode.contains(TerminalMode::LEFT_RIGHT_MARGIN)),
                1000 => Some(self.mode.contains(TerminalMode::MOUSE_TRACKING)),
                1002 => Some(self.mode.contains(TerminalMode::MOUSE_BUTTON_EVENT)),
                1003 => Some(self.mode.contains(TerminalMode::MOUSE_ANY_EVENT)),
                1004 => Some(self.mode.contains(TerminalMode::FOCUS_EVENTS)),
                1005 => Some(self.mode.contains(TerminalMode::MOUSE_UTF8)),
                1006 => Some(self.mode.contains(TerminalMode::MOUSE_SGR)),
                1007 => Some(self.mode.contains(TerminalMode::ALTERNATE_SCROLL)),
                1060 => Some(self.xterm_keyboard_type == XtermKeyboardType::Legacy),
                1061 => Some(self.xterm_keyboard_type == XtermKeyboardType::Vt220),
                2004 => Some(self.mode.contains(TerminalMode::BRACKETED_PASTE)),
                2026 => Some(self.mode.contains(TerminalMode::SYNCHRONIZED_OUTPUT)),
                _ => None,
            }
        } else {
            match number {
                4 => Some(self.mode.contains(TerminalMode::INSERT)),
                20 => Some(self.mode.contains(TerminalMode::LINEFEED_NEWLINE)),
                _ => None,
            }
        };
        enabled.map_or(0, |enabled| if enabled { 1 } else { 2 })
    }

    /// Emit one of the safe, terminal-local XTWINOPS geometry reports.
    /// Screen, outer-window, and mutation operations require app/window policy
    /// and are deliberately not handled here. Bounded application-label
    /// operations are implemented separately below.
    pub(crate) fn report_text_area_geometry(&mut self, operation: u16) {
        let response = match operation {
            14 => format!(
                "\x1b[4;{};{}t",
                self.grid_pixel_height, self.grid_pixel_width
            ),
            16 => format!(
                "\x1b[6;{};{}t",
                self.cell_pixel_height, self.cell_pixel_width
            ),
            18 => format!("\x1b[8;{};{}t", self.rows, self.cols),
            _ => return,
        };
        self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
    }

    /// Report application-authored labels for xterm XTWINOPS 20/21.
    pub(crate) fn report_application_title(&mut self, operation: u16) {
        let response = match operation {
            20 => format!("\x1b]L{}\x1b\\", self.application_titles.icon()),
            21 => format!("\x1b]l{}\x1b\\", self.application_titles.window()),
            _ => return,
        };
        self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
    }

    /// Report xterm's title-stack position and fixed capacity (CSI # S).
    pub(crate) fn report_title_stack_position(&mut self) {
        let response = format!(
            "\x1b[{};{}#S",
            self.application_titles.used(),
            XTERM_TITLE_STACK_DEPTH
        );
        self.push_event(TerminalEvent::PtyWrite(response.into_bytes()));
    }

    /// DECRQPSR — report cursor/rendition/character-set state or horizontal
    /// tab stops using the VT420 DECCIR/DECTABSR DCS formats.
    pub(crate) fn request_presentation_state(&mut self, report: u16) {
        let response = match report {
            1 => self.cursor_information_report(),
            2 => self.tab_stop_report(),
            _ => return,
        };
        self.push_event(TerminalEvent::PtyWrite(response));
    }

    fn cursor_information_report(&self) -> Vec<u8> {
        let cursor = self.grid().cursor();
        let mut rendition = b'@';
        rendition |= u8::from(self.current_attrs.contains(CellAttrs::INVERSE)) << 3;
        rendition |= u8::from(
            self.current_attrs
                .intersects(CellAttrs::BLINK | CellAttrs::RAPID_BLINK),
        ) << 2;
        rendition |= u8::from(self.current_attrs.has_underline()) << 1;
        rendition |= u8::from(self.current_attrs.contains(CellAttrs::BOLD));

        let protection = b'@' | u8::from(self.current_character_protected);
        let mut flags = b'@';
        flags |= u8::from(cursor.wrap_pending) << 3;
        flags |= u8::from(self.single_shift == Some(3)) << 2;
        flags |= u8::from(self.single_shift == Some(2)) << 1;
        flags |= u8::from(self.mode.contains(TerminalMode::ORIGIN));

        let designators: String = self
            .g_sets
            .iter()
            .map(|charset| char::from(charset.designator()))
            .collect();
        format!(
            "\x1bP1$u{};{};1;{};{};{};{};{};@;{}\x1b\\",
            cursor.row + 1,
            cursor.col + 1,
            char::from(rendition),
            char::from(protection),
            char::from(flags),
            self.gl_register,
            self.gr_register,
            designators,
        )
        .into_bytes()
    }

    fn tab_stop_report(&self) -> Vec<u8> {
        let mut response = b"\x1bP2$u".to_vec();
        for (column, _) in self.tabs.iter().enumerate().filter(|(_, set)| **set) {
            if response.len() > b"\x1bP2$u".len() {
                response.push(b'/');
            }
            response.extend_from_slice((column + 1).to_string().as_bytes());
        }
        response.extend_from_slice(b"\x1b\\");
        response
    }

    /// DECRSPS — restore a previously reported cursor or tab-stop state.
    /// Parsing is deliberately atomic: malformed or unsupported data leaves
    /// the complete live state unchanged.
    pub(crate) fn restore_presentation_state(&mut self, report: u8, data: &[u8]) {
        match report {
            1 => {
                let Some(state) = CursorPresentationState::parse(data, self.rows, self.cols) else {
                    return;
                };
                self.grid_mut().set_cursor(state.row, state.col);
                self.grid_mut().set_wrap_pending(state.wrap_pending);
                self.current_attrs.remove(
                    CellAttrs::BOLD
                        | CellAttrs::BLINK
                        | CellAttrs::RAPID_BLINK
                        | CellAttrs::INVERSE
                        | CellAttrs::ALL_UNDERLINES,
                );
                self.current_attrs.set(CellAttrs::BOLD, state.bold);
                self.current_attrs
                    .set(CellAttrs::UNDERLINE, state.underline);
                self.current_attrs.set(CellAttrs::BLINK, state.blink);
                self.current_attrs.set(CellAttrs::INVERSE, state.inverse);
                self.current_character_protected = state.protected;
                self.set_mode(TerminalMode::ORIGIN, state.origin_mode);
                self.g_sets = state.g_sets;
                self.gl_register = state.gl_register;
                self.gr_register = state.gr_register;
                self.single_shift = state.single_shift;
            }
            2 => {
                let Some(tabs) = parse_tab_stop_report(data, self.cols) else {
                    return;
                };
                self.tabs = tabs;
            }
            _ => {}
        }
    }

    /// Return the setting body used by a successful DECRQSS response.
    pub(crate) fn decrqss_status(&self, request: &[u8]) -> Option<Vec<u8>> {
        match request {
            b"m" => Some(self.sgr_status()),
            b"r" => Some(format!("{};{}r", self.scroll_top + 1, self.scroll_bottom).into_bytes()),
            b"s" => Some(format!("{};{}s", self.scroll_left + 1, self.scroll_right).into_bytes()),
            b" q" => Some(format!("{} q", self.cursor_style_parameter()).into_bytes()),
            b"\"q" => Some(format!("{}\"q", self.character_protection_parameter()).into_bytes()),
            b"\"p" => Some(
                format!(
                    "{};{}\"p",
                    self.dec_conformance_level + 60,
                    u8::from(!self.use_8bit_controls)
                )
                .into_bytes(),
            ),
            b"*x" => Some(format!("{}*x", self.attribute_change_extent_parameter()).into_bytes()),
            // Xterm answers these legacy page/screen geometry settings even
            // when its window-operation policy denies the corresponding
            // mutation controls. Preserve its DECSLPP 24-line floor and
            // max-row convention; Termeric has no selectable 132-column mode,
            // so the report remains at that setting's 80-column default.
            b"t" => Some(format!("{}t", self.rows.saturating_sub(1).max(24)).into_bytes()),
            b"$|" => Some(b"80$|".to_vec()),
            b"*|" => Some(format!("{}*|", self.rows).into_bytes()),
            b">0m" => Some(format!(">0;{}m", self.xterm_keyboard_resource.modify()).into_bytes()),
            b">0f" => Some(format!(">0;{}f", self.xterm_keyboard_resource.format()).into_bytes()),
            b">4f" => Some(
                format!(">4;{}f", self.xterm_modify_other_keys.format().parameter()).into_bytes(),
            ),
            // The shared-shape resources 1-3 and 5-7.
            [b'>', number, kind @ (b'm' | b'f')] => {
                let number = char::from(*number).to_digit(10)?;
                let resource = XtermKeyResource::from_number(number as u16)?;
                let setting = self.xterm_key_resources.get(resource);
                let value = if *kind == b'm' {
                    setting.mode().parameter()
                } else {
                    u16::from(setting.format().parameter())
                };
                Some(format!(">{number};{value}{}", char::from(*kind)).into_bytes())
            }
            _ => None,
        }
    }
}
