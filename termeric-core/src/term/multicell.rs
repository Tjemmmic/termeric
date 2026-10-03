//! Bounded parser for Kitty's OSC 66 text-sizing protocol.

use crate::grid::MultiCellAlignment;

/// Maximum UTF-8 payload allowed by the protocol.
pub(crate) const MAX_OSC66_TEXT_BYTES: usize = 4_096;
const MAX_OSC66_METADATA_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Osc66Command {
    pub scale: u8,
    /// Logical width in scaled cells. Zero requests grapheme segmentation.
    pub width: u8,
    pub numerator: u8,
    pub denominator: u8,
    pub vertical: MultiCellAlignment,
    pub horizontal: MultiCellAlignment,
    pub text: String,
}

impl Osc66Command {
    /// Parse parameters after the OSC 66 selector. The terminal parser splits
    /// OSC fields at semicolons, so the text tail is joined back together.
    pub(super) fn parse(params: &[&[u8]]) -> Option<Self> {
        let metadata = *params.first()?;
        if metadata.len() > MAX_OSC66_METADATA_BYTES {
            return None;
        }

        let mut command = Self {
            scale: 1,
            width: 0,
            numerator: 0,
            denominator: 0,
            vertical: MultiCellAlignment::Start,
            horizontal: MultiCellAlignment::Start,
            text: String::new(),
        };
        let mut vertical = 0_u8;
        let mut horizontal = 0_u8;
        let mut seen = 0_u8;
        if !metadata.is_empty() {
            for pair in metadata.split(|byte| *byte == b':') {
                let equals = pair.iter().position(|byte| *byte == b'=')?;
                let (key, value_with_equals) = pair.split_at(equals);
                let value = value_with_equals.get(1..)?;
                if key.len() != 1 || value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
                    return None;
                }
                let number = std::str::from_utf8(value).ok()?.parse::<u8>().ok()?;
                let (bit, slot, maximum) = match key[0] {
                    b's' => (1 << 0, &mut command.scale, 7),
                    b'w' => (1 << 1, &mut command.width, 7),
                    b'n' => (1 << 2, &mut command.numerator, 15),
                    b'd' => (1 << 3, &mut command.denominator, 15),
                    b'v' => (1 << 4, &mut vertical, 2),
                    b'h' => (1 << 5, &mut horizontal, 2),
                    _ => return None,
                };
                if seen & bit != 0 || number > maximum {
                    return None;
                }
                seen |= bit;
                *slot = number;
            }
        }
        if command.scale == 0
            || (command.denominator == 0 && command.numerator != 0)
            || (command.denominator != 0 && command.numerator >= command.denominator)
        {
            return None;
        }

        command.vertical = parse_alignment(vertical)?;
        command.horizontal = parse_alignment(horizontal)?;

        let text_len = params
            .get(1..)?
            .iter()
            .try_fold(0_usize, |total, field| total.checked_add(field.len()))?
            .checked_add(params.len().saturating_sub(2))?;
        if text_len == 0 || text_len > MAX_OSC66_TEXT_BYTES {
            return None;
        }
        let mut text = Vec::with_capacity(text_len);
        for (index, field) in params[1..].iter().enumerate() {
            if index != 0 {
                text.push(b';');
            }
            text.extend_from_slice(field);
        }
        let text = std::str::from_utf8(&text).ok()?;
        if text.chars().any(is_unsafe_text_scalar) {
            return None;
        }
        command.text.push_str(text);
        Some(command)
    }
}

fn parse_alignment(value: u8) -> Option<MultiCellAlignment> {
    match value {
        0 => Some(MultiCellAlignment::Start),
        1 => Some(MultiCellAlignment::End),
        2 => Some(MultiCellAlignment::Center),
        _ => None,
    }
}

fn is_unsafe_text_scalar(character: char) -> bool {
    let value = character as u32;
    character.is_control() || (0xfdd0..=0xfdef).contains(&value) || value & 0xffff >= 0xfffe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults_metadata_and_semicolon_text() {
        let parsed =
            Osc66Command::parse(&[b"s=2:w=3:n=1:d=2:v=1:h=2", b"a", b"b"]).expect("valid OSC 66");
        assert_eq!(parsed.scale, 2);
        assert_eq!(parsed.width, 3);
        assert_eq!(parsed.numerator, 1);
        assert_eq!(parsed.denominator, 2);
        assert_eq!(parsed.vertical, MultiCellAlignment::End);
        assert_eq!(parsed.horizontal, MultiCellAlignment::Center);
        assert_eq!(parsed.text, "a;b");
    }

    #[test]
    fn accepts_every_defined_field_value_and_default() {
        let defaults = Osc66Command::parse(&[b"", b"x"]).unwrap();
        assert_eq!((defaults.scale, defaults.width), (1, 0));
        assert_eq!((defaults.numerator, defaults.denominator), (0, 0));
        assert_eq!(defaults.vertical, MultiCellAlignment::Start);
        assert_eq!(defaults.horizontal, MultiCellAlignment::Start);

        for scale in 1..=7 {
            assert!(Osc66Command::parse(&[format!("s={scale}").as_bytes(), b"x"]).is_some());
        }
        for width in 0..=7 {
            assert!(Osc66Command::parse(&[format!("w={width}").as_bytes(), b"x"]).is_some());
        }
        for numerator in 0..15 {
            for denominator in (numerator + 1)..=15 {
                let metadata = format!("n={numerator}:d={denominator}");
                assert!(Osc66Command::parse(&[metadata.as_bytes(), b"x"]).is_some());
            }
        }
        for vertical in 0..=2 {
            for horizontal in 0..=2 {
                let metadata = format!("n=1:d=2:v={vertical}:h={horizontal}");
                assert!(Osc66Command::parse(&[metadata.as_bytes(), b"x"]).is_some());
            }
        }
    }

    #[test]
    fn rejects_duplicate_unknown_invalid_and_unsafe_metadata() {
        for params in [
            vec![b"s=1:s=2".as_slice(), b"x".as_slice()],
            vec![b"q=1".as_slice(), b"x".as_slice()],
            vec![b"s=0".as_slice(), b"x".as_slice()],
            vec![b"n=2:d=2".as_slice(), b"x".as_slice()],
            vec![b"v=3".as_slice(), b"x".as_slice()],
            vec![b"s=-1".as_slice(), b"x".as_slice()],
            vec![b"".as_slice(), b"x\n".as_slice()],
            vec![b"".as_slice(), "\u{fdd0}".as_bytes()],
            vec![b"".as_slice(), "\u{10ffff}".as_bytes()],
        ] {
            assert!(Osc66Command::parse(&params).is_none(), "{params:?}");
        }
    }

    #[test]
    fn enforces_text_byte_limit_before_allocation() {
        let maximum = vec![b'x'; MAX_OSC66_TEXT_BYTES];
        assert!(Osc66Command::parse(&[b"", &maximum]).is_some());
        let oversized = vec![b'x'; MAX_OSC66_TEXT_BYTES + 1];
        assert!(Osc66Command::parse(&[b"", &oversized]).is_none());

        let oversized_metadata = vec![b's'; MAX_OSC66_METADATA_BYTES + 1];
        assert!(Osc66Command::parse(&[&oversized_metadata, b"x"]).is_none());
        assert!(Osc66Command::parse(&[b"", b"\xff"]).is_none());
    }
}
