//! Wrapped, styled chrome text, measured by the renderer and laid out by UI
//! code.
//!
//! UI layout needs to know how tall a paragraph is before anything is drawn,
//! and hit testing needs the same answer, so measurement sits behind
//! [`TextMeasure`]. The renderer implements it with the desktop's UI fonts;
//! tests use a fixed-advance measurer, which gives every character the same
//! width.

use std::borrow::Cow;
use std::sync::Arc;

use unicode_segmentation::UnicodeSegmentation;

const ELLIPSIS: &str = "…";

/// A face of the desktop's UI fonts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct UiFace {
    /// The monospace UI face instead of the sans-serif one.
    pub mono: bool,
    pub bold: bool,
    pub italic: bool,
}

impl UiFace {
    pub const SANS: UiFace = UiFace {
        mono: false,
        bold: false,
        italic: false,
    };
    pub const SANS_BOLD: UiFace = UiFace {
        mono: false,
        bold: true,
        italic: false,
    };
    pub const MONO: UiFace = UiFace {
        mono: true,
        bold: false,
        italic: false,
    };
}

/// A run of text in one face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextSpan<'a> {
    pub text: &'a str,
    pub face: UiFace,
}

/// Text size and line spacing in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextSize {
    pub font_px: f32,
    pub line_px: f32,
}

/// One laid-out run: text from a single span on a single line.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidRun {
    /// Offset from the paragraph's left edge.
    pub x: f32,
    pub width: f32,
    pub text: String,
    /// Index of the span the text came from.
    pub span: usize,
    /// Byte offset of `text` in the paragraph's spans joined together.
    pub start: usize,
}

/// One line of a paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct LaidLine {
    /// Offset of the line's top from the paragraph's top.
    pub top: f32,
    /// Offset of the baseline from the paragraph's top.
    pub baseline: f32,
    pub height: f32,
    pub runs: Vec<LaidRun>,
}

/// A paragraph wrapped to a width.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayout {
    pub lines: Vec<LaidLine>,
    /// Width of the widest line.
    pub width: f32,
    pub height: f32,
}

/// Measures and wraps UI text.
pub trait TextMeasure {
    /// Lays `spans` out, wrapping at `width` when given. Hard newlines always
    /// start a new line. An empty paragraph has one empty line.
    fn layout(
        &mut self,
        spans: &[TextSpan<'_>],
        size: TextSize,
        width: Option<f32>,
    ) -> Arc<TextLayout>;

    /// Advance width of one line of text.
    fn width(&mut self, text: &str, face: UiFace, font_px: f32) -> f32;
}

/// The longest grapheme prefix of `text` that fits `max_width` once an
/// ellipsis is appended, or `text` itself when it already fits. Returns an
/// empty string when not even the ellipsis fits.
pub fn fit_with_ellipsis(
    text: &str,
    max_width: f32,
    measure: impl FnMut(&str) -> f32,
) -> Cow<'_, str> {
    fit(text, max_width, measure, Keep::Start)
}

/// Like [`fit_with_ellipsis`], but keeps the longest grapheme suffix behind a
/// leading ellipsis, for text whose end matters most, such as a draft being
/// typed.
pub fn fit_end_with_ellipsis(
    text: &str,
    max_width: f32,
    measure: impl FnMut(&str) -> f32,
) -> Cow<'_, str> {
    fit(text, max_width, measure, Keep::End)
}

#[derive(Clone, Copy)]
enum Keep {
    Start,
    End,
}

fn fit(
    text: &str,
    max_width: f32,
    mut measure: impl FnMut(&str) -> f32,
    keep: Keep,
) -> Cow<'_, str> {
    if text.is_empty() || max_width <= 0.0 {
        return Cow::Borrowed("");
    }
    if measure(text) <= max_width {
        return Cow::Borrowed(text);
    }
    if measure(ELLIPSIS) > max_width {
        return Cow::Borrowed("");
    }
    // `starts[k]` is where grapheme `k` begins: the byte length of the
    // first `k` graphemes.
    let starts: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(start, _)| start)
        .collect();
    let candidate = |kept: usize| match keep {
        Keep::Start => format!("{}{ELLIPSIS}", text[..starts[kept]].trim_end()),
        Keep::End => {
            let from = starts
                .get(starts.len() - kept)
                .copied()
                .unwrap_or(text.len());
            format!("{ELLIPSIS}{}", text[from..].trim_start())
        }
    };
    // Invariant: `low` graphemes fit; `high` graphemes do not.
    let (mut low, mut high) = (0, starts.len());
    while high - low > 1 {
        let middle = (low + high) / 2;
        if measure(&candidate(middle)) <= max_width {
            low = middle;
        } else {
            high = middle;
        }
    }
    Cow::Owned(candidate(low))
}

/// A measurer where every character advances by the same amount, for tests.
#[cfg(test)]
#[derive(Clone, Copy, Debug)]
pub struct FixedMeasure {
    pub advance: f32,
}

#[cfg(test)]
impl TextMeasure for FixedMeasure {
    fn layout(
        &mut self,
        spans: &[TextSpan<'_>],
        size: TextSize,
        width: Option<f32>,
    ) -> Arc<TextLayout> {
        let columns = width.map(|width| ((width / self.advance).floor() as usize).max(1));
        // (character, span, byte offset) in paragraph order; hard line
        // breaks stay as '\n'.
        let mut characters: Vec<(char, usize, usize)> = Vec::new();
        let mut offset = 0;
        for (index, span) in spans.iter().enumerate() {
            for character in span.text.chars() {
                characters.push((character, index, offset));
                offset += character.len_utf8();
            }
        }
        let mut lines: Vec<Vec<(char, usize, usize)>> = vec![Vec::new()];
        let mut index = 0;
        while index < characters.len() {
            let (character, _, _) = characters[index];
            if character == '\n' {
                lines.push(Vec::new());
                index += 1;
                continue;
            }
            // The next word, including the whitespace after it.
            let mut end = index;
            while end < characters.len()
                && characters[end].0 != '\n'
                && !characters[end].0.is_whitespace()
            {
                end += 1;
            }
            while end < characters.len()
                && characters[end].0 != '\n'
                && characters[end].0.is_whitespace()
            {
                end += 1;
            }
            let word = &characters[index..end];
            let current = lines.last_mut().expect("at least one line");
            let visible = word.iter().filter(|(c, _, _)| !c.is_whitespace()).count();
            if let Some(columns) = columns {
                if !current.is_empty() && current.len() + visible > columns {
                    lines.push(Vec::new());
                }
                // A word longer than a line breaks anywhere.
                for &entry in word {
                    let line = lines.last_mut().expect("at least one line");
                    if line.len() >= columns && !entry.0.is_whitespace() {
                        lines.push(Vec::new());
                    }
                    lines.last_mut().expect("at least one line").push(entry);
                }
            } else {
                current.extend_from_slice(word);
            }
            index = end;
        }
        let mut layout = TextLayout::default();
        for (number, line) in lines.iter().enumerate() {
            let top = number as f32 * size.line_px;
            let mut runs: Vec<LaidRun> = Vec::new();
            for (column, &(character, span, start)) in line.iter().enumerate() {
                match runs.last_mut() {
                    Some(run) if run.span == span => {
                        run.text.push(character);
                        run.width += self.advance;
                    }
                    _ => runs.push(LaidRun {
                        x: column as f32 * self.advance,
                        width: self.advance,
                        text: character.to_string(),
                        span,
                        start,
                    }),
                }
            }
            let trimmed = line
                .iter()
                .rev()
                .skip_while(|(c, _, _)| c.is_whitespace())
                .count();
            layout.width = layout.width.max(trimmed as f32 * self.advance);
            layout.lines.push(LaidLine {
                top,
                baseline: top + size.font_px,
                height: size.line_px,
                runs,
            });
        }
        layout.height = layout.lines.len() as f32 * size.line_px;
        Arc::new(layout)
    }

    fn width(&mut self, text: &str, _face: UiFace, _font_px: f32) -> f32 {
        text.chars().count() as f32 * self.advance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: TextSize = TextSize {
        font_px: 10.0,
        line_px: 14.0,
    };

    fn texts(layout: &TextLayout) -> Vec<String> {
        layout
            .lines
            .iter()
            .map(|line| {
                line.runs
                    .iter()
                    .map(|run| run.text.as_str())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn fixed_measure_wraps_words_breaks_long_ones_and_keeps_spans() {
        let mut measure = FixedMeasure { advance: 1.0 };
        let layout = measure.layout(
            &[
                TextSpan {
                    text: "the quick ",
                    face: UiFace::SANS,
                },
                TextSpan {
                    text: "brown",
                    face: UiFace::MONO,
                },
                TextSpan {
                    text: " foxes\nabcdefghijkl",
                    face: UiFace::SANS,
                },
            ],
            SIZE,
            Some(10.0),
        );
        assert_eq!(
            texts(&layout),
            ["the quick ", "brown ", "foxes", "abcdefghij", "kl"]
        );
        assert_eq!(layout.lines[1].runs[0].span, 1);
        assert_eq!(layout.lines[1].runs[0].start, 10);
        assert_eq!(layout.lines[1].runs[1].span, 2);
        assert_eq!(layout.lines[3].runs[0].start, 22);
        assert_eq!(layout.height, 5.0 * 14.0);
        assert_eq!(layout.width, 10.0);
        assert_eq!(layout.lines[2].top, 28.0);
        assert_eq!(layout.lines[2].baseline, 38.0);
    }

    #[test]
    fn unwrapped_and_empty_paragraphs_have_their_natural_lines() {
        let mut measure = FixedMeasure { advance: 2.0 };
        let layout = measure.layout(
            &[TextSpan {
                text: "one two",
                face: UiFace::SANS,
            }],
            SIZE,
            None,
        );
        assert_eq!(texts(&layout), ["one two"]);
        assert_eq!(layout.width, 14.0);
        let empty = measure.layout(&[], SIZE, Some(50.0));
        assert_eq!(empty.lines.len(), 1);
        assert_eq!(empty.height, 14.0);
    }

    /// Ten pixels per grapheme, like a monospace face.
    fn width(text: &str) -> f32 {
        text.graphemes(true).count() as f32 * 10.0
    }

    #[test]
    fn text_that_fits_is_returned_unchanged() {
        assert_eq!(fit_with_ellipsis("claude", 60.0, width), "claude");
        assert_eq!(fit_with_ellipsis("", 60.0, width), "");
    }

    #[test]
    fn long_text_keeps_the_longest_prefix_that_fits_with_an_ellipsis() {
        assert_eq!(fit_with_ellipsis("tjemmmic@emrakul", 60.0, width), "tjemm…");
        assert_eq!(fit_with_ellipsis("tjemmmic@emrakul", 20.0, width), "t…");
        assert_eq!(fit_with_ellipsis("tjemmmic@emrakul", 15.0, width), "…");
        assert_eq!(fit_with_ellipsis("tjemmmic@emrakul", 10.0, width), "…");
        assert_eq!(fit_with_ellipsis("tjemmmic@emrakul", 9.0, width), "");
        assert_eq!(fit_with_ellipsis("tjemmmic@emrakul", 0.0, width), "");
    }

    #[test]
    fn truncation_never_splits_a_grapheme_or_leaves_trailing_space() {
        let family = "👩‍👩‍👧 family";
        assert_eq!(fit_with_ellipsis(family, 30.0, width), "👩‍👩‍👧…");
        assert_eq!(fit_with_ellipsis("ab cdef", 40.0, width), "ab…");
        assert_eq!(fit_with_ellipsis("e\u{301}tude", 30.0, width), "e\u{301}t…");
    }

    #[test]
    fn keeping_the_end_cuts_the_start_behind_an_ellipsis() {
        assert_eq!(fit_end_with_ellipsis("claude", 60.0, width), "claude");
        assert_eq!(
            fit_end_with_ellipsis("tjemmmic@emrakul", 60.0, width),
            "…rakul"
        );
        assert_eq!(fit_end_with_ellipsis("tjemmmic@emrakul", 20.0, width), "…l");
        assert_eq!(fit_end_with_ellipsis("tjemmmic@emrakul", 10.0, width), "…");
        assert_eq!(fit_end_with_ellipsis("tjemmmic@emrakul", 9.0, width), "");
        assert_eq!(fit_end_with_ellipsis("family 👩‍👩‍👧", 30.0, width), "…👩‍👩‍👧");
        assert_eq!(fit_end_with_ellipsis("abcd ef", 40.0, width), "…ef");
    }
}
