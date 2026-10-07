//! Shared pieces of the chrome's cards and dialogs: raised surfaces,
//! shadows, card surfaces, scrollbars, buttons, and focus rings.

use super::Renderer;
use super::text::TextClip;
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::constants::*;
use crate::ui::geometry::Bounds;
use crate::ui::text_input_viewport::TextInputViewport;

/// The colors of a field's text and of its placeholder.
#[derive(Clone, Copy)]
pub(super) struct FieldTextColors {
    pub(super) text: [f32; 4],
    pub(super) placeholder: [f32; 4],
}

impl Renderer {
    /// A raised chrome surface: soft `shadows` (each a logical spread and
    /// its alpha) dropped `drop` logical pixels, a hairline `border`, and the
    /// menu background inside it, all faded by `opacity`. `radius` is
    /// logical. The modal frame, anchored menus, and the find box are drawn
    /// with it.
    pub(super) fn push_raised_surface(
        &mut self,
        bounds: Bounds,
        radius: f32,
        shadows: &[(f32, f32)],
        drop: f32,
        border: [f32; 4],
        opacity: f32,
    ) {
        let Bounds {
            x,
            y,
            width,
            height,
        } = bounds;
        let radius = self.px(radius);
        let drop = self.px(drop);
        for &(spread, alpha) in shadows {
            let spread = self.px(spread);
            self.rect_pipeline.push_rounded_rect(
                x - spread,
                y - spread / 2.0 + drop,
                width + spread * 2.0,
                height + spread * 2.0,
                [0.0, 0.0, 0.0, alpha * opacity],
                radius + spread,
            );
        }
        // The border is the surface's own edge: a full rounded rectangle in
        // its color with the background inset a hairline inside it.
        let line = self.px(BORDER_WIDTH);
        self.rect_pipeline.push_rounded_rect(
            x,
            y,
            width,
            height,
            scale_alpha(border, opacity),
            radius,
        );
        self.rect_pipeline.push_rounded_rect(
            x + line,
            y + line,
            width - line * 2.0,
            height - line * 2.0,
            scale_alpha(self.ui.menu_bg, opacity),
            radius - line,
        );
    }

    /// A soft shadow under a card, `layers` rounded rectangles spreading
    /// down and out with falling alpha.
    pub(super) fn draw_card_shadow(&mut self, card: Bounds, alpha: f32, layers: u32) {
        let radius = self.px(CARD_CORNER_RADIUS);
        let step = self.px(4.0);
        for layer in 0..layers {
            let spread = (layer as f32 + 1.0) * step;
            self.rect_pipeline.push_rounded_rect(
                card.x - spread,
                card.y - spread * 0.5 + spread * 0.8,
                card.width + spread * 2.0,
                card.height + spread * 1.5,
                [0.0, 0.0, 0.0, alpha / (layer as f32 + 1.0)],
                radius + spread,
            );
        }
    }

    /// A rounded card surface with a hairline border.
    pub(super) fn draw_card(&mut self, card: Bounds, background: [f32; 4], border: [f32; 4]) {
        let radius = self.px(CARD_CORNER_RADIUS);
        self.rect_pipeline.push_rounded_rect(
            card.x,
            card.y,
            card.width,
            card.height,
            background,
            radius,
        );
        self.rect_pipeline.push_rounded_outline(
            card.x,
            card.y,
            card.width,
            card.height,
            border,
            radius,
            self.px(BORDER_WIDTH),
        );
    }

    /// A filter field's surface: a rounded `fill` with a magnifier in
    /// `icon` color in the slot before its query text.
    pub(super) fn draw_filter_field(&mut self, field: Bounds, fill: [f32; 4], icon: [f32; 4]) {
        self.rect_pipeline.push_rounded_rect(
            field.x,
            field.y,
            field.width,
            field.height,
            fill,
            self.px(BUTTON_CORNER_RADIUS),
        );
        self.push_magnifier(
            field.x + self.cell_width() * FILTER_ICON_CENTER_CELLS,
            field.y + field.height / 2.0,
            icon,
            fill,
        );
    }

    /// A single-line field's text through `view`, clipped to `field`:
    /// `text`, or `placeholder` while it is empty, and a caret in `caret`'s
    /// color when it is given.
    pub(super) fn draw_field_text(
        &mut self,
        field: Bounds,
        view: &TextInputViewport,
        text: &str,
        placeholder: &str,
        colors: FieldTextColors,
        caret: Option<[f32; 4]>,
    ) {
        let cell_width = self.cell_width();
        let clip = TextClip {
            left: view.text_x,
            top: field.y,
            right: view.right,
            bottom: field.bottom(),
        };
        if text.is_empty() {
            // The placeholder starts where the text would, in the same space.
            if let Some(hint) = TextInputViewport::calculate(
                placeholder,
                0,
                view.text_x,
                view.text_y,
                view.right,
                cell_width,
            ) {
                self.text_pipeline.push_ui_run(
                    &placeholder[hint.text_range],
                    view.text_x,
                    view.text_y,
                    colors.placeholder,
                    cell_width,
                    clip,
                );
            }
        } else {
            self.text_pipeline.push_ui_run(
                &text[view.text_range.clone()],
                view.text_x,
                view.text_y,
                colors.text,
                cell_width,
                clip,
            );
        }
        if let Some(color) = caret {
            self.rect_pipeline.push_rect(
                view.cursor_x,
                view.text_y,
                self.px(2.0).min(view.right - view.cursor_x),
                self.cell_height(),
                color,
            );
        }
    }

    /// A card's vertical scrollbar: a faint track and an accent thumb for
    /// `visible` of `total` rows with `offset` scrolled past at the top;
    /// nothing while everything fits.
    pub(super) fn draw_scrollbar(
        &mut self,
        track: Bounds,
        visible: usize,
        total: usize,
        offset: usize,
        anim: f32,
    ) {
        if visible == 0 || total <= visible {
            return;
        }
        self.rect_pipeline.push_rounded_rect(
            track.x,
            track.y,
            track.width,
            track.height,
            with_alpha(self.ui.palette_border, 0.15 * anim),
            track.width / 2.0,
        );
        self.draw_scroll_thumb(
            track,
            visible,
            total,
            offset,
            with_alpha(self.ui.palette_accent, 0.5 * anim),
        );
    }

    /// The rounded thumb every scrollbar draws along `track`, for `visible`
    /// of `total` rows with `offset` scrolled past at the top; nothing
    /// while everything fits.
    pub(super) fn draw_scroll_thumb(
        &mut self,
        track: Bounds,
        visible: usize,
        total: usize,
        offset: usize,
        color: [f32; 4],
    ) {
        let min_height = self.px(MIN_SCROLL_THUMB_HEIGHT);
        if let Some(thumb) = scroll_thumb(track, visible, total, offset, min_height) {
            self.rect_pipeline.push_rounded_rect(
                thumb.x,
                thumb.y,
                thumb.width,
                thumb.height,
                color,
                thumb.width / 2.0,
            );
        }
    }

    /// A dialog button in the accent color, its label centred in the UI
    /// face. `primary` raises the surface; hover and press tint it further.
    pub(super) fn draw_button(
        &mut self,
        button: Bounds,
        label: &str,
        primary: bool,
        hovered: bool,
        pressed: bool,
        anim: f32,
    ) {
        let base_alpha = if primary { 0.12 } else { 0.05 };
        let background_alpha = if pressed {
            base_alpha + 0.18
        } else if hovered {
            base_alpha + 0.10
        } else {
            base_alpha
        };
        // A press sinks the button by a pixel on every side.
        let button = if pressed {
            button.inset(self.px(1.0), self.px(1.0))
        } else {
            button
        };
        let accent = self.ui.palette_accent;
        let radius = self.px(BUTTON_CORNER_RADIUS);
        self.rect_pipeline.push_rounded_rect(
            button.x,
            button.y,
            button.width,
            button.height,
            with_alpha(accent, background_alpha * anim),
            radius,
        );
        let border_alpha = if hovered { 0.65 } else { 0.35 };
        self.rect_pipeline.push_rounded_outline(
            button.x,
            button.y,
            button.width,
            button.height,
            with_alpha(accent, border_alpha * anim),
            radius,
            self.px(BORDER_WIDTH),
        );
        if hovered && !pressed {
            let glow = self.px(2.0);
            self.rect_pipeline.push_rounded_rect(
                button.x - glow,
                button.y - glow,
                button.width + glow * 2.0,
                button.height + glow * 2.0,
                with_alpha(accent, 0.06 * anim),
                radius + glow,
            );
        }
        let text_alpha = if hovered { 1.0 } else { 0.8 };
        let size = self.ui_label_px();
        self.push_ui_label_centered(
            label,
            button.center()[0],
            button.y,
            button.height,
            button.width,
            size,
            with_alpha(accent, text_alpha * anim),
        );
    }

    /// A two-pixel keyboard focus ring just inside `bounds`.
    pub(super) fn draw_focus_ring(&mut self, bounds: Bounds, color: [f32; 4]) {
        self.rect_pipeline.push_rounded_outline(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            color,
            self.px(BUTTON_CORNER_RADIUS),
            self.px(2.0),
        );
    }
}

/// Where a scrollbar's thumb sits along `track` for `visible` of `total`
/// rows with `offset` scrolled past at the top: its share of the track, at
/// least `min_height` (to stay findable in long scrollback) and on whole
/// pixels. None while everything fits.
pub(super) fn scroll_thumb(
    track: Bounds,
    visible: usize,
    total: usize,
    offset: usize,
    min_height: f32,
) -> Option<Bounds> {
    if visible == 0 || total <= visible || track.height <= 0.0 {
        return None;
    }
    let max_offset = total - visible;
    let height = (track.height * visible as f32 / total as f32)
        .max(min_height)
        .round()
        .min(track.height);
    let travel = track.height - height;
    let y = track.y + travel * offset.min(max_offset) as f32 / max_offset as f32;
    Some(Bounds::new(track.x, y.round(), track.width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scroll_thumb_spans_its_share_and_travels_the_whole_track() {
        let track = Bounds::new(10.0, 100.0, 4.0, 200.0);
        assert_eq!(scroll_thumb(track, 10, 10, 0, 24.0), None);
        assert_eq!(scroll_thumb(track, 0, 10, 0, 24.0), None);

        // A quarter of the rows is a quarter of the track, from its top to
        // its bottom as the offset runs to the last row.
        let top = scroll_thumb(track, 10, 40, 0, 24.0).unwrap();
        assert_eq!(top, Bounds::new(10.0, 100.0, 4.0, 50.0));
        let bottom = scroll_thumb(track, 10, 40, 30, 24.0).unwrap();
        assert_eq!(bottom.bottom(), track.bottom());
        let middle = scroll_thumb(track, 10, 40, 15, 24.0).unwrap();
        assert_eq!(middle.y, 175.0);
        // An offset past the end stays at the end.
        assert_eq!(scroll_thumb(track, 10, 40, 99, 24.0), Some(bottom));
    }

    #[test]
    fn a_scroll_thumb_keeps_its_minimum_and_whole_pixels() {
        let track = Bounds::new(0.0, 0.5, 3.0, 300.0);
        let thumb = scroll_thumb(track, 1, 10_000, 1, 24.0).unwrap();
        assert_eq!(thumb.height, 24.0);
        assert_eq!(thumb.y, thumb.y.round());
        // A track shorter than the minimum is filled, never overrun.
        let short = Bounds::new(0.0, 0.0, 3.0, 20.0);
        assert_eq!(scroll_thumb(short, 1, 3, 0, 24.0).unwrap().height, 20.0);
    }
}
