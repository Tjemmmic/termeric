//! Encoding for terminal-controlled mouse, focus, paste, and wheel input.

use termeric_core::term::TerminalMode;
use winit::keyboard::ModifiersState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseReportButton {
    Left,
    Middle,
    Right,
}

impl MouseReportButton {
    fn code(self) -> u16 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WheelDirection {
    Up,
    Down,
    Left,
    Right,
}

impl WheelDirection {
    fn code(self) -> u16 {
        match self {
            Self::Up => 64,
            Self::Down => 65,
            Self::Left => 66,
            Self::Right => 67,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseReportKind {
    Press(MouseReportButton),
    Release(MouseReportButton),
    Motion(Option<MouseReportButton>),
    Wheel(WheelDirection),
}

/// Encode one terminal mouse report. Coordinates are zero-based grid cells;
/// terminal protocols receive their one-based equivalents.
pub(crate) fn encode_mouse_report(
    mode: TerminalMode,
    kind: MouseReportKind,
    col: usize,
    row: usize,
    modifiers: ModifiersState,
) -> Option<Vec<u8>> {
    if !mouse_kind_enabled(mode, kind) {
        return None;
    }

    let x = col.checked_add(1)?;
    let y = row.checked_add(1)?;
    let modifier_code = u16::from(modifiers.shift_key()) * 4
        + u16::from(modifiers.alt_key()) * 8
        + u16::from(modifiers.control_key()) * 16;
    let (base_code, sgr_release) = match kind {
        MouseReportKind::Press(button) => (button.code(), false),
        MouseReportKind::Release(button) => (button.code(), true),
        MouseReportKind::Motion(button) => (button.map_or(3, MouseReportButton::code) + 32, false),
        MouseReportKind::Wheel(direction) => (direction.code(), false),
    };
    let code = base_code + modifier_code;

    if mode.contains(TerminalMode::MOUSE_SGR) {
        let final_byte = if sgr_release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{x};{y}{final_byte}").into_bytes());
    }

    // Legacy release reports use button 3 rather than the released button.
    let legacy_code = if sgr_release { 3 + modifier_code } else { code };
    let mut bytes = Vec::with_capacity(12);
    bytes.extend_from_slice(b"\x1b[M");
    bytes.push(u8::try_from(legacy_code + 32).ok()?);
    let utf8 = mode.contains(TerminalMode::MOUSE_UTF8);
    append_legacy_value(&mut bytes, x + 32, utf8)?;
    append_legacy_value(&mut bytes, y + 32, utf8)?;
    Some(bytes)
}

fn mouse_kind_enabled(mode: TerminalMode, kind: MouseReportKind) -> bool {
    if mode.contains(TerminalMode::MOUSE_ANY_EVENT) {
        return true;
    }
    if mode.contains(TerminalMode::MOUSE_BUTTON_EVENT) {
        return !matches!(kind, MouseReportKind::Motion(None));
    }
    if mode.contains(TerminalMode::MOUSE_TRACKING) {
        return !matches!(kind, MouseReportKind::Motion(_));
    }
    if mode.contains(TerminalMode::MOUSE_X10) {
        return matches!(kind, MouseReportKind::Press(_) | MouseReportKind::Wheel(_));
    }
    false
}

fn append_legacy_value(bytes: &mut Vec<u8>, value: usize, utf8: bool) -> Option<()> {
    if utf8 {
        // DECSET 1005 is defined only through terminal coordinate 2015.
        if !(33..=2047).contains(&value) {
            return None;
        }
        let character = char::from_u32(u32::try_from(value).ok()?)?;
        let mut buffer = [0; 4];
        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    } else {
        if !(33..=255).contains(&value) {
            return None;
        }
        bytes.push(u8::try_from(value).ok()?);
    }
    Some(())
}

pub(crate) fn encode_focus_report(mode: TerminalMode, focused: bool) -> Option<&'static [u8]> {
    mode.contains(TerminalMode::FOCUS_EVENTS)
        .then_some(if focused { b"\x1b[I" } else { b"\x1b[O" })
}

/// Frame a clipboard payload as one ordered PTY write when bracketed paste is active.
pub(crate) fn encode_paste(mode: TerminalMode, text: &str) -> Vec<u8> {
    if !mode.contains(TerminalMode::BRACKETED_PASTE) {
        return text.as_bytes().to_vec();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

/// DECSET 1007 converts a wheel step to a cursor key only in the alternate screen.
pub(crate) fn encode_wheel_as_cursor_key(
    mode: TerminalMode,
    direction: WheelDirection,
) -> Option<&'static [u8]> {
    if !mode.contains(TerminalMode::ALT_SCREEN | TerminalMode::ALTERNATE_SCROLL) {
        return None;
    }
    let application = mode.contains(TerminalMode::CURSOR_KEYS_APPLICATION);
    match (direction, application) {
        (WheelDirection::Up, true) => Some(b"\x1bOA"),
        (WheelDirection::Down, true) => Some(b"\x1bOB"),
        (WheelDirection::Up, false) => Some(b"\x1b[A"),
        (WheelDirection::Down, false) => Some(b"\x1b[B"),
        (WheelDirection::Left | WheelDirection::Right, _) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(shift: bool, alt: bool, control: bool) -> ModifiersState {
        let mut result = ModifiersState::empty();
        result.set(ModifiersState::SHIFT, shift);
        result.set(ModifiersState::ALT, alt);
        result.set(ModifiersState::CONTROL, control);
        result
    }

    #[test]
    fn default_encoding_covers_press_release_wheel_and_modifiers() {
        let mode = TerminalMode::MOUSE_TRACKING;
        assert_eq!(
            encode_mouse_report(
                mode,
                MouseReportKind::Press(MouseReportButton::Left),
                0,
                0,
                ModifiersState::empty(),
            ),
            Some(b"\x1b[M !!".to_vec())
        );
        assert_eq!(
            encode_mouse_report(
                mode,
                MouseReportKind::Release(MouseReportButton::Right),
                4,
                2,
                modifiers(true, true, true),
            ),
            Some(vec![0x1b, b'[', b'M', 63, 37, 35])
        );
        assert_eq!(
            encode_mouse_report(
                TerminalMode::MOUSE_X10,
                MouseReportKind::Wheel(WheelDirection::Down),
                9,
                4,
                ModifiersState::empty(),
            ),
            Some(vec![0x1b, b'[', b'M', 97, 42, 37])
        );
    }

    #[test]
    fn tracking_modes_filter_motion_and_release() {
        let motion = MouseReportKind::Motion(Some(MouseReportButton::Left));
        let no_button = MouseReportKind::Motion(None);
        assert_eq!(
            encode_mouse_report(
                TerminalMode::MOUSE_X10,
                MouseReportKind::Release(MouseReportButton::Left),
                0,
                0,
                ModifiersState::empty(),
            ),
            None
        );
        assert_eq!(
            encode_mouse_report(
                TerminalMode::MOUSE_TRACKING,
                motion,
                0,
                0,
                ModifiersState::empty(),
            ),
            None
        );
        assert!(
            encode_mouse_report(
                TerminalMode::MOUSE_BUTTON_EVENT,
                motion,
                0,
                0,
                ModifiersState::empty(),
            )
            .is_some()
        );
        assert_eq!(
            encode_mouse_report(
                TerminalMode::MOUSE_BUTTON_EVENT,
                no_button,
                0,
                0,
                ModifiersState::empty(),
            ),
            None
        );
        assert!(
            encode_mouse_report(
                TerminalMode::MOUSE_ANY_EVENT,
                no_button,
                0,
                0,
                ModifiersState::empty(),
            )
            .is_some()
        );
    }

    #[test]
    fn sgr_encoding_is_unbounded_and_distinguishes_release() {
        let mode = TerminalMode::MOUSE_TRACKING | TerminalMode::MOUSE_SGR;
        assert_eq!(
            encode_mouse_report(
                mode,
                MouseReportKind::Press(MouseReportButton::Right),
                49_999,
                69_999,
                modifiers(false, true, true),
            ),
            Some(b"\x1b[<26;50000;70000M".to_vec())
        );
        assert_eq!(
            encode_mouse_report(
                mode,
                MouseReportKind::Release(MouseReportButton::Right),
                2,
                3,
                ModifiersState::empty(),
            ),
            Some(b"\x1b[<2;3;4m".to_vec())
        );
    }

    #[test]
    fn utf8_encoding_extends_coordinates_but_rejects_unsafe_values() {
        let utf8 = TerminalMode::MOUSE_TRACKING | TerminalMode::MOUSE_UTF8;
        assert_eq!(
            encode_mouse_report(
                TerminalMode::MOUSE_TRACKING,
                MouseReportKind::Press(MouseReportButton::Left),
                223,
                0,
                ModifiersState::empty(),
            ),
            None
        );
        let report = encode_mouse_report(
            utf8,
            MouseReportKind::Press(MouseReportButton::Left),
            223,
            0,
            ModifiersState::empty(),
        )
        .unwrap();
        assert_eq!(&report[..4], b"\x1b[M ");
        assert_eq!(&report[4..6], "Ā".as_bytes());
        assert_eq!(report[6], b'!');
        assert_eq!(
            encode_mouse_report(
                utf8,
                MouseReportKind::Press(MouseReportButton::Left),
                2015,
                0,
                ModifiersState::empty(),
            ),
            None
        );
    }

    #[test]
    fn focus_paste_and_alternate_scroll_are_mode_gated() {
        assert_eq!(encode_focus_report(TerminalMode::default(), true), None);
        assert_eq!(
            encode_focus_report(TerminalMode::FOCUS_EVENTS, true),
            Some(b"\x1b[I".as_slice())
        );
        assert_eq!(
            encode_focus_report(TerminalMode::FOCUS_EVENTS, false),
            Some(b"\x1b[O".as_slice())
        );
        assert_eq!(encode_paste(TerminalMode::default(), "a\nb"), b"a\nb");
        assert_eq!(
            encode_paste(TerminalMode::BRACKETED_PASTE, "a\nb"),
            b"\x1b[200~a\nb\x1b[201~"
        );
        let alt_scroll = TerminalMode::ALT_SCREEN | TerminalMode::ALTERNATE_SCROLL;
        assert_eq!(
            encode_wheel_as_cursor_key(alt_scroll, WheelDirection::Up),
            Some(b"\x1b[A".as_slice())
        );
        assert_eq!(
            encode_wheel_as_cursor_key(
                alt_scroll | TerminalMode::CURSOR_KEYS_APPLICATION,
                WheelDirection::Down,
            ),
            Some(b"\x1bOB".as_slice())
        );
    }
}
