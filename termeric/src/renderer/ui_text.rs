//! Application-chrome text in the desktop's sans-serif UI face.

use std::borrow::Cow;
use std::sync::Arc;

use super::glyph_cache::GlyphCache;
use super::text::TextClip;
use super::{Renderer, physical_font_size};
pub(super) use crate::ui::text_layout::fit_with_ellipsis;
use crate::ui::text_layout::{TextLayout, TextMeasure, TextSize, TextSpan, UiFace};

/// Chrome label size in points, the common desktop UI default.
pub(super) const UI_LABEL_POINTS: f32 = 10.0;

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
        let (fitted, width) = self.fit_ui_label(text, max_width, size_px);
        if fitted.is_empty() {
            return 0.0;
        }
        let baseline = self
            .glyph_cache
            .ui_font_metrics(size_px)
            .centered_baseline(top, height);
        self.text_pipeline
            .push_ui_text(&fitted, x.round(), baseline, size_px, color, None);
        width
    }

    /// A card's heading: one line in the bold UI face at the dialog heading
    /// size, from `x` and centred in the box from `top` with `height`
    /// pixels, shortened with an ellipsis to fit `max_width`.
    pub(super) fn push_card_heading(
        &mut self,
        text: &str,
        x: f32,
        top: f32,
        height: f32,
        max_width: f32,
        color: [f32; 4],
    ) {
        let size_px = physical_font_size(crate::ui::dialog::HEADING_POINTS, self.scale_factor);
        let face = UiFace::SANS_BOLD;
        let glyphs = &mut self.glyph_cache;
        let fitted = fit_with_ellipsis(text, max_width, |candidate| {
            glyphs.ui_text_width_in(candidate, face, size_px)
        });
        if fitted.is_empty() {
            return;
        }
        let baseline = self
            .glyph_cache
            .ui_font_metrics(size_px)
            .centered_baseline(top, height);
        self.text_pipeline.push_ui_text_in(
            &fitted,
            face,
            x.round(),
            baseline,
            size_px,
            color,
            None,
        );
    }

    /// [`Renderer::push_ui_label`] clipped to `clip`, for a label in a
    /// scrolled region that may sit partly outside it.
    pub(super) fn push_ui_label_clipped(
        &mut self,
        text: &str,
        x: f32,
        top: f32,
        height: f32,
        max_width: f32,
        size_px: f32,
        color: [f32; 4],
        clip: TextClip,
    ) -> f32 {
        let (fitted, width) = self.fit_ui_label(text, max_width, size_px);
        if fitted.is_empty() {
            return 0.0;
        }
        let baseline = self
            .glyph_cache
            .ui_font_metrics(size_px)
            .centered_baseline(top, height);
        self.text_pipeline
            .push_ui_text(&fitted, x.round(), baseline, size_px, color, Some(clip));
        width
    }

    /// `text` as [`Renderer::push_ui_label`] draws it in `max_width`, and the
    /// width it takes, so a caret after a label lands where the label ends.
    pub(super) fn fit_ui_label<'a>(
        &mut self,
        text: &'a str,
        max_width: f32,
        size_px: f32,
    ) -> (Cow<'a, str>, f32) {
        let glyphs = &mut self.glyph_cache;
        let fitted = fit_with_ellipsis(text, max_width, |candidate| {
            glyphs.ui_text_width(candidate, size_px)
        });
        let width = if fitted.is_empty() {
            0.0
        } else {
            self.glyph_cache.ui_text_width(&fitted, size_px)
        };
        (fitted, width)
    }

    /// [`Renderer::push_ui_label`] centred on `center_x`.
    pub(super) fn push_ui_label_centered(
        &mut self,
        text: &str,
        center_x: f32,
        top: f32,
        height: f32,
        max_width: f32,
        size_px: f32,
        color: [f32; 4],
    ) -> f32 {
        let width = self.ui_text_width(text, size_px).min(max_width);
        self.push_ui_label(
            text,
            center_x - width / 2.0,
            top,
            height,
            max_width,
            size_px,
            color,
        )
    }
}

impl TextMeasure for GlyphCache {
    fn layout(
        &mut self,
        spans: &[TextSpan<'_>],
        size: TextSize,
        width: Option<f32>,
    ) -> Arc<TextLayout> {
        self.layout_ui_text(spans, size, width)
    }

    fn width(&mut self, text: &str, face: UiFace, font_px: f32) -> f32 {
        self.ui_text_width_in(text, face, font_px)
    }
}

impl Renderer {
    /// Measures and wraps chrome text in the desktop's UI faces.
    pub fn text_measure(&mut self) -> &mut impl TextMeasure {
        &mut self.glyph_cache
    }

    /// Draws a laid-out paragraph with its top-left corner at (`x`, `y`).
    /// `paint` gives each span's face and color.
    pub(super) fn push_text_layout(
        &mut self,
        layout: &TextLayout,
        x: f32,
        y: f32,
        font_px: f32,
        paint: impl Fn(usize) -> (UiFace, [f32; 4]),
        clip: Option<TextClip>,
    ) {
        for line in &layout.lines {
            let baseline = (y + line.baseline).round();
            if let Some(clip) = clip
                && (baseline + line.height < clip.top || baseline - line.height > clip.bottom)
            {
                continue;
            }
            for run in &line.runs {
                let (face, color) = paint(run.span);
                self.text_pipeline.push_ui_text_in(
                    &run.text,
                    face,
                    (x + run.x).round(),
                    baseline,
                    font_px,
                    color,
                    clip,
                );
            }
        }
    }
}

impl Renderer {
    /// Like [`Renderer::push_ui_label`], drawing the characters whose
    /// indices appear in `highlights` in `highlight` instead of `color`.
    /// Each run is placed at the measured width of the text before it, so
    /// highlighted and plain runs line up with an unhighlighted rendering.
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
