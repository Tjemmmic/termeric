//! Application-chrome text in the desktop's sans-serif UI face.

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

use super::{Renderer, physical_font_size};

/// Chrome label size in points, the common desktop UI default.
pub(super) const UI_LABEL_POINTS: f32 = 10.0;

const ELLIPSIS: &str = "…";

impl Renderer {
    /// Physical pixel size of chrome labels at the current display scale.
    pub(super) fn ui_label_px(&self) -> f32 {
        physical_font_size(UI_LABEL_POINTS, self.scale_factor)
    }

    /// Advance width of `text` in the UI face.
    pub(super) fn ui_text_width(&mut self, text: &str, size_px: f32) -> f32 {
        self.glyph_cache.ui_text_width(text, size_px)
    }

    /// Draw one line of UI text from `x`, centred vertically in the box from
    /// `top` with `height` pixels and shortened with an ellipsis to fit
    /// `max_width`. Returns the width actually drawn.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_ui_label(
        &mut self,
        text: &str,
        x: f32,
        top: f32,
        height: f32,
        max_width: f32,
        size_px: f32,
        color: [f32; 4],
    ) -> f32 {
        let glyphs = &mut self.glyph_cache;
        let fitted = fit_with_ellipsis(text, max_width, |candidate| {
            glyphs.ui_text_width(candidate, size_px)
        });
        if fitted.is_empty() {
            return 0.0;
        }
        let width = self.glyph_cache.ui_text_width(&fitted, size_px);
        let baseline = self
            .glyph_cache
            .ui_font_metrics(size_px)
            .centered_baseline(top, height);
        self.text_pipeline
            .push_ui_text(&fitted, x.round(), baseline, size_px, color, None);
        width
    }
}

impl Renderer {
    /// Like [`Renderer::push_ui_label`], drawing the characters whose
    /// indices appear in `highlights` in `highlight` instead of `color`.
    /// Each run is placed at the measured width of the text before it, so
    /// highlighted and plain runs line up with an unhighlighted rendering.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_ui_label_highlighted(
        &mut self,
        text: &str,
        highlights: &[usize],
        x: f32,
        top: f32,
        height: f32,
        max_width: f32,
        size_px: f32,
        color: [f32; 4],
        highlight: [f32; 4],
    ) -> f32 {
        if highlights.is_empty() {
            return self.push_ui_label(text, x, top, height, max_width, size_px, color);
        }
        let glyphs = &mut self.glyph_cache;
        let fitted = fit_with_ellipsis(text, max_width, |candidate| {
            glyphs.ui_text_width(candidate, size_px)
        })
        .into_owned();
        if fitted.is_empty() {
            return 0.0;
        }
        // An appended ellipsis is never part of the match.
        let kept = if fitted == text {
            fitted.chars().count()
        } else {
            fitted.chars().count() - 1
        };
        let baseline = self
            .glyph_cache
            .ui_font_metrics(size_px)
            .centered_baseline(top, height);
        let x = x.round();
        let mut run_start = 0;
        let mut run_highlighted = None;
        let boundaries = fitted
            .char_indices()
            .enumerate()
            .map(|(index, (byte, _))| (byte, index < kept && highlights.contains(&index)))
            .chain(std::iter::once((fitted.len(), false)));
        for (byte, highlighted) in boundaries {
            if run_highlighted == Some(highlighted) && byte < fitted.len() {
                continue;
            }
            if let Some(was_highlighted) = run_highlighted
                && byte > run_start
            {
                let offset = self
                    .glyph_cache
                    .ui_text_width(&fitted[..run_start], size_px);
                self.text_pipeline.push_ui_text(
                    &fitted[run_start..byte],
                    (x + offset).round(),
                    baseline,
                    size_px,
                    if was_highlighted { highlight } else { color },
                    None,
                );
            }
            run_start = byte;
            run_highlighted = Some(highlighted);
        }
        self.glyph_cache.ui_text_width(&fitted, size_px)
    }
}

/// The longest grapheme prefix of `text` that fits `max_width` once an
/// ellipsis is appended, or `text` itself when it already fits. Returns an
/// empty string when not even the ellipsis fits.
pub(super) fn fit_with_ellipsis(
    text: &str,
    max_width: f32,
    mut measure: impl FnMut(&str) -> f32,
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
    // `ends[k]` is the byte length of the first `k` graphemes.
    let ends: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(start, _)| start)
        .collect();
    let candidate = |kept: usize| format!("{}{ELLIPSIS}", text[..ends[kept]].trim_end());
    // Invariant: `low` graphemes fit; `high` graphemes do not.
    let (mut low, mut high) = (0, ends.len());
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
