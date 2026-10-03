//! Character classes for text that crosses into one line of chrome, a
//! single-line field, or a protocol field. Terminal grid content is never
//! filtered with these.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
