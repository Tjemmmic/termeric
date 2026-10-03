//! Terminal Unicode policy.
//!
//! Grapheme boundaries follow `unicode-segmentation` 1.13.3 (Unicode 17.0).
//! Display width follows `unicode-width` 0.2 (Unicode 17.0), with East Asian
//! ambiguous characters kept narrow. Width is clamped to the terminal cell
//! model's supported range of zero through two columns.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Return the narrow-policy terminal width for one extended grapheme cluster.
pub fn grapheme_width(grapheme: &str) -> usize {
    UnicodeWidthStr::width(grapheme).min(2)
}

/// Whether appending `next` keeps `existing` as one extended grapheme cluster.
pub(crate) fn extends_grapheme(existing: &str, next: char) -> bool {
    let mut combined = String::with_capacity(existing.len() + next.len_utf8());
    combined.push_str(existing);
    combined.push(next);
    UnicodeSegmentation::graphemes(combined.as_str(), true).count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_versions_are_explicit() {
        assert_eq!(unicode_segmentation::UNICODE_VERSION, (17, 0, 0));
        assert_eq!(unicode_width::UNICODE_VERSION, (17, 0, 0));
    }

    #[test]
    fn representative_cluster_widths() {
        for (text, width) in [
            ("a", 1),
            ("界", 2),
            ("e\u{301}", 1),
            ("·", 1),
            ("☺\u{fe0e}", 1),
            ("☺\u{fe0f}", 2),
            ("🇺🇸", 2),
            ("👍🏽", 2),
            ("👩\u{200d}💻", 2),
        ] {
            assert_eq!(grapheme_width(text), width, "{text:?}");
        }
    }
}
