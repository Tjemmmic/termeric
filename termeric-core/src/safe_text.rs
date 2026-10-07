//! Character classes for text that crosses into one line of chrome, a
//! single-line field, or a protocol field, and cutting such text on character
//! boundaries. Terminal grid content is never filtered with these.

/// Breaks a single line: C0/C1 controls and the Unicode line and paragraph
/// separators.
pub fn breaks_single_line(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

/// A bidirectional formatting character. Displayed, it can reorder the text
/// around it, so untrusted text could pose as something else.
pub fn is_bidi_format(character: char) -> bool {
    matches!(
        character,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// Unsafe in one displayed line of untrusted text: it breaks the line or
/// reorders what surrounds it.
pub fn is_unsafe_display_char(character: char) -> bool {
    breaks_single_line(character) || is_bidi_format(character)
}

/// Unsafe in multi-line untrusted text, which keeps its newlines and tabs:
/// any other control character, or bidi formatting.
pub fn is_unsafe_multiline_char(character: char) -> bool {
    (character.is_control() && !matches!(character, '\n' | '\t')) || is_bidi_format(character)
}

/// The largest character boundary in `text` at or before `index`, which is
/// `text.len()` past its end. `str::floor_char_boundary` once the MSRV has it.
pub fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    (0..=index)
        .rev()
        .find(|&offset| text.is_char_boundary(offset))
        .unwrap_or(0)
}

/// The smallest character boundary in `text` at or after `index`, which is
/// `text.len()` past its end. `str::ceil_char_boundary` once the MSRV has it.
pub fn ceil_char_boundary(text: &str, index: usize) -> usize {
    (index.min(text.len())..text.len())
        .find(|&offset| text.is_char_boundary(offset))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_boundaries_round_down_or_up_and_stop_at_the_end() {
        let text = "aé界";
        assert_eq!(floor_char_boundary(text, 0), 0);
        assert_eq!(floor_char_boundary(text, 2), 1);
        assert_eq!(floor_char_boundary(text, 4), 3);
        assert_eq!(floor_char_boundary(text, 6), 6);
        assert_eq!(floor_char_boundary(text, 99), 6);
        assert_eq!(ceil_char_boundary(text, 0), 0);
        assert_eq!(ceil_char_boundary(text, 2), 3);
        assert_eq!(ceil_char_boundary(text, 4), 6);
        assert_eq!(ceil_char_boundary(text, 99), 6);
        assert_eq!(floor_char_boundary("", 3), 0);
        assert_eq!(ceil_char_boundary("", 3), 0);
    }

    #[test]
    fn line_breaks_and_bidi_formatting_are_separate_classes() {
        for character in ['\n', '\r', '\t', '\u{7f}', '\u{85}', '\u{2028}', '\u{2029}'] {
            assert!(breaks_single_line(character), "{character:?}");
            assert!(!is_bidi_format(character), "{character:?}");
            assert!(is_unsafe_display_char(character), "{character:?}");
        }
        for character in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202e}', '\u{2066}', '\u{2069}',
        ] {
            assert!(is_bidi_format(character), "{character:?}");
            assert!(!breaks_single_line(character), "{character:?}");
            assert!(is_unsafe_display_char(character), "{character:?}");
        }
        for character in ['a', ' ', 'é', '界', '\u{200d}', '\u{202f}', '\u{206a}'] {
            assert!(!is_unsafe_display_char(character), "{character:?}");
        }
    }

    #[test]
    fn multiline_text_keeps_newlines_and_tabs_only() {
        for character in ['\n', '\t', 'a', ' ', '界', '\u{2028}'] {
            assert!(!is_unsafe_multiline_char(character), "{character:?}");
        }
        for character in ['\r', '\u{1b}', '\u{7f}', '\u{85}', '\u{202e}', '\u{2066}'] {
            assert!(is_unsafe_multiline_char(character), "{character:?}");
        }
    }
}
