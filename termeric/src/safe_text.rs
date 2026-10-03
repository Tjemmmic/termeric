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

/// `text` without the characters [`is_unsafe_display_char`] rejects, cut at
/// a character boundary to at most `max_bytes` UTF-8 bytes. For text from
/// another process shown in one line: errors, titles, and metadata.
pub fn single_line_within(text: &str, max_bytes: usize) -> String {
    let mut output = String::with_capacity(text.len().min(max_bytes));
    for character in text
        .chars()
        .filter(|character| !is_unsafe_display_char(*character))
    {
        if output.len() + character.len_utf8() > max_bytes {
            break;
        }
        output.push(character);
    }
    output
}

/// At most `max_chars` characters of `text`; when it is cut, the last kept
/// character becomes `…`.
pub fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    let mut characters = text.chars();
    let mut kept: String = characters.by_ref().take(max_chars).collect();
    if characters.next().is_some() && max_chars > 0 {
        kept.pop();
        kept.push('…');
    }
    kept
}

/// One displayed line of untrusted text: a tab becomes `tab_width` spaces
/// and any other unsafe character U+FFFD, so nothing is hidden or reordered,
/// within `max_chars` characters as [`truncate_with_ellipsis`] cuts them.
pub fn display_line(text: &str, max_chars: usize, tab_width: usize) -> String {
    let mut characters = text.chars().flat_map(|character| {
        let (shown, count) = match character {
            '\t' => (' ', tab_width),
            character if is_unsafe_display_char(character) => ('\u{fffd}', 1),
            character => (character, 1),
        };
        std::iter::repeat_n(shown, count)
    });
    let mut line: String = characters.by_ref().take(max_chars).collect();
    if characters.next().is_some() && max_chars > 0 {
        line.pop();
        line.push('…');
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_within_strips_unsafe_text_and_cuts_on_a_character_boundary() {
        assert_eq!(single_line_within("a\u{1b}[31mb\nc", 64), "a[31mbc");
        assert_eq!(single_line_within("ab\u{202e}cd\u{2066}", 64), "abcd");
        assert_eq!(single_line_within("ééé", 5), "éé");
        assert_eq!(single_line_within("", 5), "");
    }

    #[test]
    fn truncation_keeps_the_limit_including_the_ellipsis() {
        assert_eq!(truncate_with_ellipsis("abcdef", 4), "abc…");
        assert_eq!(truncate_with_ellipsis("abcd", 4), "abcd");
        assert_eq!(truncate_with_ellipsis("界界界", 2), "界…");
        assert_eq!(truncate_with_ellipsis("abc", 0), "");
    }

    #[test]
    fn display_lines_expand_tabs_and_show_hidden_characters() {
        assert_eq!(display_line("a\tb", 10, 4), "a    b");
        assert_eq!(display_line("a\tb", 10, 1), "a b");
        assert_eq!(display_line("x\u{202e}y\u{7}", 10, 4), "x\u{fffd}y\u{fffd}");
        assert_eq!(display_line("ab\tcd", 4, 4), "ab …");
        assert_eq!(display_line("abcd", 4, 4), "abcd");
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
}
