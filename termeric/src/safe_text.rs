//! Bounding untrusted text for chrome and protocols, on top of the character
//! classes from `termeric_core::safe_text`. Terminal grid content is never
//! filtered with these.

pub use termeric_core::safe_text::{
    breaks_single_line, ceil_char_boundary, floor_char_boundary, is_bidi_format,
    is_unsafe_display_char,
};

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

/// The longest prefix of `text` within `max_bytes` UTF-8 bytes.
pub fn prefix_within(text: &str, max_bytes: usize) -> &str {
    &text[..floor_char_boundary(text, max_bytes)]
}

/// At most `max_chars` characters of `text`; when it is cut, the last kept
/// character becomes `…`, after any whitespace the cut left at the end.
pub fn truncate_with_ellipsis(text: &str, max_chars: usize) -> String {
    let mut characters = text.chars();
    let mut kept: String = characters.by_ref().take(max_chars).collect();
    if characters.next().is_some() && max_chars > 0 {
        kept.pop();
        kept.truncate(kept.trim_end().len());
        kept.push('…');
    }
    kept
}

/// The first line of `text` that is not blank, trimmed, within `max_chars`
/// characters as [`truncate_with_ellipsis`] cuts them: how a message or
/// command is quoted in a title, log, or notification.
pub fn first_line_truncated(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    truncate_with_ellipsis(line, max_chars)
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
        assert_eq!(truncate_with_ellipsis("ab cdef", 4), "ab…");
    }

    #[test]
    fn prefixes_stay_within_their_bytes_on_a_character_boundary() {
        assert_eq!(prefix_within("aé界", 2), "a");
        assert_eq!(prefix_within("aé界", 3), "aé");
        assert_eq!(prefix_within("aé界", 99), "aé界");
    }

    #[test]
    fn first_lines_skip_blank_lines_and_are_trimmed_and_cut() {
        assert_eq!(first_line_truncated("\n  \n  first  \nsecond", 10), "first");
        assert_eq!(first_line_truncated("abcdefgh\nmore", 4), "abc…");
        assert_eq!(first_line_truncated("  \n", 4), "");
    }

    #[test]
    fn display_lines_expand_tabs_and_show_hidden_characters() {
        assert_eq!(display_line("a\tb", 10, 4), "a    b");
        assert_eq!(display_line("a\tb", 10, 1), "a b");
        assert_eq!(display_line("x\u{202e}y\u{7}", 10, 4), "x\u{fffd}y\u{fffd}");
        assert_eq!(display_line("ab\tcd", 4, 4), "ab …");
        assert_eq!(display_line("abcd", 4, 4), "abcd");
    }
}
