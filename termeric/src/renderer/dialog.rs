//! Dialog rendering: the shared modal frame over the scrim, the heading in
//! the UI face, the scrolled body clipped to its viewport, and the button
//! row. Every position comes from the `DialogLayout` the app also hit-tests,
//! places the IME caret, and builds accessibility bounds with.

use super::Renderer;
use super::draw::{ButtonState, FieldTextColors};
use super::text::TextClip;
use super::types::DialogInfo;
use crate::ui::color::{scale_alpha, with_alpha};
use crate::ui::constants::{BORDER_WIDTH, BUTTON_CORNER_RADIUS};
use crate::ui::dialog::{DialogBlock, DialogFocus, DialogHit};
use crate::ui::text_layout::UiFace;

impl Renderer {
    pub(super) fn build_dialog(
        &mut self,
        info: &DialogInfo,
        window_width: f32,
        window_height: f32,
    ) {
        let anim = info.animation;
        let layout = &info.layout;
        let offset = info.scroll;
        let fade = |color: [f32; 4]| scale_alpha(color, anim);

        self.rect_pipeline.push_rect(
            0.0,
            0.0,
            window_width,
            window_height,
            fade(self.ui.palette_scrim),
        );
        let card = layout.card;
        self.build_modal_frame(card.x, card.y, card.width, card.height, anim);

        let heading = fade(self.ui.palette_text);
        self.push_text_layout(
            &layout.heading,
            layout.heading_bounds.x,
            layout.heading_bounds.y,
            layout.heading_size.font_px,
            |_| (UiFace::SANS_BOLD, heading),
            None,
        );

        let hovered = info
            .mouse_pos
            .filter(|_| info.awaiting)
            .and_then(|(x, y)| layout.hit(x, y, offset));
        let focus = info.focus.filter(|_| info.awaiting);

        let clip = TextClip::from(layout.body);
        let outer_clip = self.rect_pipeline.set_clip(Some(clip));
        self.build_dialog_blocks(info, clip);
        self.build_dialog_field(info, focus, clip);
        self.rect_pipeline.set_clip(outer_clip);

        if let Some(track) = layout.scrollbar {
            self.draw_scroll_thumb(
                track,
                layout.body.height as usize,
                layout.content_height as usize,
                offset as usize,
                fade(scale_alpha(self.ui.menu_text_dim, 0.6)),
            );
        }

        for (index, (bounds, button)) in layout.buttons.iter().zip(&info.buttons).enumerate() {
            let hovered = hovered == Some(DialogHit::Button(index));
            self.draw_button(
                *bounds,
                &button.label,
                button.role,
                ButtonState {
                    hovered,
                    pressed: hovered && info.mouse_pressed,
                    focused: focus == Some(DialogFocus::Button(index)),
                    disabled: false,
                },
                anim,
            );
        }
    }

    /// Paragraphs, values on their inset surface, and errors.
    fn build_dialog_blocks(&mut self, info: &DialogInfo, clip: TextClip) {
        let layout = &info.layout;
        let anim = info.animation;
        for (index, (block, content)) in layout.blocks.iter().zip(&info.body).enumerate() {
            let Some(bounds) = layout.block(index, info.scroll) else {
                continue;
            };
            if layout.visible(bounds).is_none() {
                continue;
            }
            let (face, color, size) = match content {
                DialogBlock::Text(_) => (UiFace::SANS, self.ui.dialog_text, layout.body_size),
                DialogBlock::Error(_) => (UiFace::SANS, self.ui.error, layout.body_size),
                DialogBlock::Value(_) => (UiFace::MONO, self.ui.palette_text, layout.value_size),
            };
            if block.inset {
                self.rect_pipeline.push_rounded_rect(
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    scale_alpha(self.ui.inset_bg, anim),
                    self.px(BUTTON_CORNER_RADIUS),
                );
            }
            let color = scale_alpha(color, anim);
            self.push_text_layout(
                &block.text,
                bounds.x + block.text_x,
                bounds.y + block.text_y,
                size.font_px,
                |_| (face, color),
                Some(clip),
            );
        }
    }

    /// The field's label, its inset box with the text and caret, and its
    /// error.
    fn build_dialog_field(
        &mut self,
        info: &DialogInfo,
        focus: Option<DialogFocus>,
        clip: TextClip,
    ) {
        let layout = &info.layout;
        let anim = info.animation;
        let (Some(field), Some(label), Some(input)) = (
            info.field.as_ref(),
            layout.field_label(info.scroll),
            layout.field_box(info.scroll),
        ) else {
            return;
        };
        let focused = focus == Some(DialogFocus::Field);
        self.push_ui_label_clipped(
            &field.label,
            label.x,
            label.y,
            label.height,
            label.width,
            self.ui_label_px(),
            scale_alpha(self.ui.dialog_text, anim),
            clip,
        );
        let radius = self.px(BUTTON_CORNER_RADIUS);
        self.rect_pipeline.push_rounded_rect(
            input.x,
            input.y,
            input.width,
            input.height,
            scale_alpha(self.ui.inset_bg, anim),
            radius,
        );
        self.rect_pipeline.push_rounded_outline(
            input.x,
            input.y,
            input.width,
            input.height,
            scale_alpha(self.ui.menu_border, anim),
            radius,
            self.px(BORDER_WIDTH),
        );
        // The text clips to the part of the box inside the body, as the
        // caret clips to the body itself.
        if let Some(view) = layout.field_viewport(&field.value, field.cursor_byte, info.scroll)
            && let Some(shown) = layout.visible(input)
        {
            self.draw_field_text(
                shown,
                &view,
                &field.value,
                "",
                FieldTextColors {
                    text: scale_alpha(self.ui.palette_text, anim),
                    placeholder: scale_alpha(self.ui.palette_text_dim, anim),
                },
                focused.then(|| with_alpha(self.ui.palette_accent, anim)),
            );
        }
        if focused {
            self.draw_focus_ring(
                input,
                with_alpha(self.ui.focus_ring, anim),
                BUTTON_CORNER_RADIUS,
            );
        }
        if let Some((bounds, text)) = layout.field_error(info.scroll) {
            let color = scale_alpha(self.ui.error, anim);
            self.push_text_layout(
                text,
                bounds.x,
                bounds.y,
                layout.body_size.font_px,
                |_| (UiFace::SANS, color),
                Some(clip),
            );
        }
    }
}
