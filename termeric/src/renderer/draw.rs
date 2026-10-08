//! Shared pieces of the chrome's cards and dialogs: raised surfaces,
//! shadows, card surfaces, scrollbars, buttons, and focus rings.

use super::Renderer;
use super::text::TextClip;
use crate::ui::agents_panel::AgentRowState;
use crate::ui::color::{mix, scale_alpha, with_alpha};
use crate::ui::constants::*;
use crate::ui::dialog::ButtonRole;
use crate::ui::geometry::Bounds;
use crate::ui::text_input_viewport::TextInputViewport;

/// The room a button's label keeps from its edges, in logical pixels.
const BUTTON_LABEL_INSET: f32 = 8.0;
/// How far outside a button its focus ring sits, in logical pixels: the
/// ring's stroke and a pixel of the surface behind it.
const FOCUS_RING_GAP: f32 = 3.0;

/// How a push button is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct ButtonState {
    pub(super) hovered: bool,
    pub(super) pressed: bool,
    pub(super) focused: bool,
    pub(super) disabled: bool,
}

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

    /// A round status dot of `diameter` centred on `center`, on whole
    /// pixels, filled with `fill`. With `ring_surface` it is a ring instead:
    /// its middle is punched out in the surface it sits on.
    pub(super) fn push_status_dot(
        &mut self,
        center: [f32; 2],
        diameter: f32,
        fill: [f32; 4],
        ring_surface: Option<[f32; 4]>,
    ) {
        let diameter = diameter.round();
        let (left, top) = (
            (center[0] - diameter / 2.0).round(),
            (center[1] - diameter / 2.0).round(),
        );
        self.rect_pipeline
            .push_rounded_rect(left, top, diameter, diameter, fill, diameter / 2.0);
        if let Some(surface) = ring_surface {
            let inner = (diameter - self.px(1.5) * 2.0).max(1.0);
            let inset = (diameter - inner) / 2.0;
            self.rect_pipeline.push_rounded_rect(
                left + inset,
                top + inset,
                inner,
                inner,
                surface,
                inner / 2.0,
            );
        }
    }

    /// The dot an agent's status shows, alike on its tab and its Agents
    /// panel row: a ring in the `neutral` text color while it works or
    /// idles (neither asks anything of the person), the theme's warning
    /// color while it needs the person, success when done, error when it
    /// failed, all at `neutral`'s alpha. Nothing while its status is
    /// unknown. Returns the fill and whether it is a ring.
    pub(super) fn agent_status_dot(
        &self,
        state: AgentRowState,
        neutral: [f32; 4],
    ) -> Option<([f32; 4], bool)> {
        let alpha = neutral[3];
        Some(match state {
            AgentRowState::Unknown => return None,
            AgentRowState::Idle => (scale_alpha(neutral, 0.5), true),
            AgentRowState::Working => (neutral, true),
            AgentRowState::NeedsYou => (with_alpha(self.ui.warning, alpha), false),
            AgentRowState::Done => (with_alpha(self.ui.success, alpha), false),
            AgentRowState::Failed => (with_alpha(self.ui.error, alpha), false),
        })
    }

    /// An accent line `thickness` logical pixels tall along a card's top
    /// edge, inset past its rounded corners so it never overhangs them.
    pub(super) fn draw_card_accent(&mut self, card: Bounds, color: [f32; 4], thickness: f32) {
        let inset = self.px(CARD_CORNER_RADIUS) * 2.0;
        let thickness = self.px(thickness);
        let width = card.width - inset * 2.0;
        if width > 0.0 {
            self.rect_pipeline.push_rounded_rect(
                card.x + inset,
                card.y,
                width,
                thickness,
                color,
                thickness / 2.0,
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
        let clip = TextClip::from(Bounds::new(
            view.text_x,
            field.y,
            view.right - view.text_x,
            field.height,
        ));
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

    /// The one push button of dialogs, cards, and Settings: its role's
    /// surface with the label centred in the UI face. Hover and press tint
    /// the surface toward the label; keyboard focus draws the focus ring
    /// just outside it, where it reads against filled buttons too.
    pub(super) fn draw_button(
        &mut self,
        button: Bounds,
        label: &str,
        role: ButtonRole,
        state: ButtonState,
        anim: f32,
    ) {
        let colors = self.ui.button(role);
        let tint = if state.disabled {
            0.0
        } else if state.pressed {
            0.16
        } else if state.hovered {
            0.08
        } else {
            0.0
        };
        let opacity = if state.disabled { 0.45 } else { 1.0 } * anim;
        let fill = mix(colors.fill, with_alpha(colors.text, colors.fill[3]), tint);
        let radius = self.px(BUTTON_CORNER_RADIUS);
        self.rect_pipeline.push_rounded_rect(
            button.x,
            button.y,
            button.width,
            button.height,
            scale_alpha(fill, opacity),
            radius,
        );
        if let Some(border) = colors.border {
            self.rect_pipeline.push_rounded_outline(
                button.x,
                button.y,
                button.width,
                button.height,
                scale_alpha(border, opacity),
                radius,
                self.px(BORDER_WIDTH),
            );
        }
        let inset = self.px(BUTTON_LABEL_INSET);
        self.push_ui_label_centered(
            label,
            button.center()[0],
            button.y,
            button.height,
            (button.width - inset * 2.0).max(0.0),
            self.ui_label_px(),
            scale_alpha(colors.text, opacity),
        );
        if state.focused && !state.disabled {
            let gap = self.px(FOCUS_RING_GAP);
            self.draw_focus_ring(
                Bounds::new(
                    button.x - gap,
                    button.y - gap,
                    button.width + gap * 2.0,
                    button.height + gap * 2.0,
                ),
                with_alpha(self.ui.focus_ring, anim),
                BUTTON_CORNER_RADIUS + FOCUS_RING_GAP,
            );
        }
    }

    /// An On/Off switch in `bounds`: a pill filled with the accent while on
    /// and inset while off, its knob at the end it is set to.
    pub(super) fn draw_switch(&mut self, bounds: Bounds, on: bool, anim: f32) {
        let accent = self.ui.button(ButtonRole::Suggested);
        let radius = bounds.height / 2.0;
        let track = if on { accent.fill } else { self.ui.inset_bg };
        self.rect_pipeline.push_rounded_rect(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            scale_alpha(track, anim),
            radius,
        );
        if !on {
            self.rect_pipeline.push_rounded_outline(
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                scale_alpha(self.ui.menu_border, anim),
                radius,
                self.px(BORDER_WIDTH),
            );
        }
        let inset = self.px(3.0).min(radius / 2.0);
        let knob = bounds.height - inset * 2.0;
        let knob_x = if on {
            bounds.right() - inset - knob
        } else {
            bounds.x + inset
        };
        let knob_color = if on {
            accent.text
        } else {
            self.ui.menu_text_dim
        };
        self.rect_pipeline.push_rounded_rect(
            knob_x,
            bounds.y + inset,
            knob,
            knob,
            scale_alpha(knob_color, anim),
            knob / 2.0,
        );
    }

    /// The keyboard focus ring every chrome control shows: two logical
    /// pixels just inside `bounds`, rounded by the logical `radius` of the
    /// control it surrounds.
    pub(super) fn draw_focus_ring(&mut self, bounds: Bounds, color: [f32; 4], radius: f32) {
        let stroke = self.px(2.0);
        if bounds.width <= stroke * 2.0 || bounds.height <= stroke * 2.0 {
            return;
        }
        self.rect_pipeline.push_rounded_outline(
            bounds.x,
            bounds.y,
            bounds.width,
            bounds.height,
            color,
            self.px(radius),
            stroke,
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
