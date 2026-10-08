//! A dialog's geometry, on whole physical pixels: the card, the heading,
//! the scrolled body (blocks, field, and list rows), and the right-aligned
//! button row. Drawing, hit tests, hover, the IME caret, scrolling, and
//! accessibility bounds all read the one [`DialogLayout`].
//!
//! Body positions are in content space, from the body's top; the scroll
//! offset is the dialog's own state and is applied where a body position
//! meets the screen, so the layout never changes as the body scrolls.

use std::sync::Arc;

use super::{BodyExtent, Dialog, DialogBlock, DialogFocus};
use crate::ui::geometry::Bounds;
use crate::ui::text_input_viewport::TextInputViewport;
use crate::ui::text_layout::{TextLayout, TextMeasure, TextSize, TextSpan, UiFace};

const PIXELS_PER_POINT: f32 = 96.0 / 72.0;
/// Body text is the chrome label size; the heading is 1.2× it, here and on
/// the other cards.
const BODY_POINTS: f32 = 10.0;
pub const HEADING_POINTS: f32 = 12.0;

// Logical pixels, scaled to the display.
const NARROW_WIDTH: f32 = 440.0;
const WIDE_WIDTH: f32 = 560.0;
const WINDOW_MARGIN: f32 = 24.0;
const PADDING: f32 = 20.0;
const HEADING_GAP: f32 = 8.0;
const BLOCK_GAP: f32 = 8.0;
const BUTTONS_GAP: f32 = 20.0;
const HEADING_LINE: f32 = 22.0;
const BODY_LINE: f32 = 20.0;
const VALUE_LINE: f32 = 18.0;
const VALUE_PADDING_X: f32 = 8.0;
const VALUE_PADDING_Y: f32 = 6.0;
const FIELD_LABEL_GAP: f32 = 6.0;
const FIELD_HEIGHT: f32 = 32.0;
const FIELD_TEXT_INSET: f32 = 10.0;
const FIELD_ERROR_GAP: f32 = 6.0;
const ROW_HEIGHT: f32 = 30.0;
const BUTTON_HEIGHT: f32 = 32.0;
const BUTTON_MIN_WIDTH: f32 = 88.0;
const BUTTON_PADDING: f32 = 16.0;
const BUTTON_SPACING: f32 = 8.0;
const SCROLLBAR_WIDTH: f32 = 4.0;
/// The room the body gives up beside its text once it scrolls.
const SCROLLBAR_GUTTER: f32 = 12.0;
/// How far above its place the card starts as it opens.
const OPEN_SLIDE: f32 = 12.0;

/// The window and display a dialog is laid out for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogMetrics {
    pub window_width: f32,
    pub window_height: f32,
    /// Display scale: logical to physical pixels.
    pub scale: f32,
    /// The terminal cell, which the field's text is placed by, like every
    /// other single-line field.
    pub cell_width: f32,
    pub cell_height: f32,
    /// The eased opening, 0 to 1.
    pub animation: f32,
}

/// One body block's place in the body.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockLayout {
    /// Top and height in the body's content.
    pub top: f32,
    pub height: f32,
    /// Where the text starts inside the block, from its left and top.
    pub text_x: f32,
    pub text_y: f32,
    pub text: Arc<TextLayout>,
    /// The block sits on an inset surface (a value).
    pub inset: bool,
}

/// The field's place in the body: its label line, its box, and an error
/// under it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldLayout {
    pub label_top: f32,
    pub label_height: f32,
    pub box_top: f32,
    pub box_height: f32,
    pub error: Option<(f32, Arc<TextLayout>)>,
}

/// What a point on a dialog lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogHit {
    Button(usize),
    Row(usize),
    Field,
    /// The body, between its controls.
    Body,
    /// The card, outside the body and the buttons.
    Card,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DialogLayout {
    pub card: Bounds,
    pub padding: f32,
    pub heading: Arc<TextLayout>,
    /// The heading's box: its text starts at the top-left corner.
    pub heading_bounds: Bounds,
    /// The body's viewport on screen; zero height when it is empty.
    pub body: Bounds,
    /// The width of the body's text and controls, short of the scrollbar's
    /// gutter when the body scrolls.
    pub content_width: f32,
    pub content_height: f32,
    pub blocks: Vec<BlockLayout>,
    pub field: Option<FieldLayout>,
    /// Top of the first list row in the body's content.
    pub rows_top: f32,
    pub row_height: f32,
    pub row_count: usize,
    pub buttons: Vec<Bounds>,
    /// The scrollbar's track, while the body scrolls.
    pub scrollbar: Option<Bounds>,
    pub heading_size: TextSize,
    pub body_size: TextSize,
    pub value_size: TextSize,
    /// The inset of a row's or the field's text from its left edge.
    pub control_inset: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// The parts of the body, laid out at one width.
struct Body {
    blocks: Vec<BlockLayout>,
    field: Option<FieldLayout>,
    rows_top: f32,
    height: f32,
}

impl DialogLayout {
    pub fn calculate(
        dialog: &Dialog,
        metrics: DialogMetrics,
        measure: &mut dyn TextMeasure,
    ) -> Self {
        let scale = metrics.scale.max(0.1);
        let px = |logical: f32| (logical * scale).round();
        let text_size = |points: f32, line: f32| TextSize {
            font_px: points * PIXELS_PER_POINT * scale,
            line_px: px(line).max(1.0),
        };
        let heading_size = text_size(HEADING_POINTS, HEADING_LINE);
        let body_size = text_size(BODY_POINTS, BODY_LINE);
        let value_size = text_size(BODY_POINTS, VALUE_LINE);

        let margin = px(WINDOW_MARGIN);
        let preferred = if dialog.kind().is_wide() {
            WIDE_WIDTH
        } else {
            NARROW_WIDTH
        };
        let width = px(preferred)
            .min((metrics.window_width - margin * 2.0).floor())
            .max(1.0);
        let padding = px(PADDING);
        let inner = (width - padding * 2.0).max(1.0);

        let heading = measure.layout(
            &[TextSpan {
                text: dialog.title(),
                face: UiFace::SANS_BOLD,
            }],
            heading_size,
            Some(inner),
        );
        let heading_height = heading.height.ceil().max(heading_size.line_px);

        let has_body =
            !dialog.body().is_empty() || dialog.field.is_some() || !dialog.rows().is_empty();
        let sizes = BodySizes {
            body: body_size,
            value: value_size,
            px: &px,
        };
        let button_height = px(BUTTON_HEIGHT);
        let fixed = padding * 2.0
            + heading_height
            + if has_body { px(HEADING_GAP) } else { 0.0 }
            + px(BUTTONS_GAP)
            + button_height;
        let room = (metrics.window_height - margin * 2.0 - fixed)
            .floor()
            .max(body_size.line_px);

        let mut content_width = inner;
        let mut body = lay_out_body(dialog, content_width, &sizes, measure);
        let scrolls = has_body && body.height > room;
        if scrolls {
            content_width = (inner - px(SCROLLBAR_GUTTER)).max(1.0);
            body = lay_out_body(dialog, content_width, &sizes, measure);
        }
        let body_height = if has_body { body.height.min(room) } else { 0.0 };

        let height = fixed + body_height;
        let slide = ((1.0 - metrics.animation.clamp(0.0, 1.0)) * px(OPEN_SLIDE)).round();
        let card = Bounds::new(
            ((metrics.window_width - width) / 2.0).round(),
            ((metrics.window_height - height) / 2.0).round() - slide,
            width,
            height,
        );
        let left = card.x + padding;
        let heading_bounds = Bounds::new(left, card.y + padding, inner, heading_height);
        let body_top = heading_bounds.bottom() + if has_body { px(HEADING_GAP) } else { 0.0 };
        let body_bounds = Bounds::new(left, body_top, inner, body_height);
        let buttons_top = body_bounds.bottom() + px(BUTTONS_GAP);
        let buttons = lay_out_buttons(
            dialog,
            card.right() - padding,
            buttons_top,
            inner,
            &sizes,
            measure,
        );
        let scrollbar_width = px(SCROLLBAR_WIDTH).max(1.0);
        let scrollbar = scrolls.then(|| {
            Bounds::new(
                body_bounds.right() - scrollbar_width,
                body_bounds.y,
                scrollbar_width,
                body_bounds.height,
            )
        });

        Self {
            card,
            padding,
            heading,
            heading_bounds,
            body: body_bounds,
            content_width,
            content_height: body.height,
            blocks: body.blocks,
            field: body.field,
            rows_top: body.rows_top,
            row_height: px(ROW_HEIGHT),
            row_count: dialog.rows().len(),
            buttons,
            scrollbar,
            heading_size,
            body_size,
            value_size,
            control_inset: px(FIELD_TEXT_INSET),
            cell_width: metrics.cell_width,
            cell_height: metrics.cell_height,
        }
    }

    /// What the dialog's scrolling needs to know of the body.
    pub fn body_extent(&self) -> BodyExtent {
        BodyExtent {
            viewport: self.body.height,
            content: self.content_height,
            line: self.body_size.line_px,
        }
    }

    /// Where `focus` sits in the body's content, top and bottom; buttons
    /// sit outside it.
    pub fn span(&self, focus: DialogFocus) -> Option<(f32, f32)> {
        match focus {
            DialogFocus::Field => self
                .field
                .as_ref()
                .map(|field| (field.label_top, field.box_top + field.box_height)),
            DialogFocus::Row(row) => (row < self.row_count).then(|| {
                let top = self.rows_top + row as f32 * self.row_height;
                (top, top + self.row_height)
            }),
            DialogFocus::Button(_) => None,
        }
    }

    /// A span of the body's content on screen, scrolled by `offset`, whole
    /// and unclipped.
    fn on_screen(&self, top: f32, height: f32, offset: f32) -> Bounds {
        Bounds::new(
            self.body.x,
            self.body.y + top - offset,
            self.content_width,
            height,
        )
    }

    /// The part of `bounds` inside the body's viewport.
    pub fn visible(&self, bounds: Bounds) -> Option<Bounds> {
        bounds.intersect(self.body)
    }

    pub fn block(&self, index: usize, offset: f32) -> Option<Bounds> {
        let block = self.blocks.get(index)?;
        Some(self.on_screen(block.top, block.height, offset))
    }

    pub fn field_label(&self, offset: f32) -> Option<Bounds> {
        let field = self.field.as_ref()?;
        Some(self.on_screen(field.label_top, field.label_height, offset))
    }

    pub fn field_box(&self, offset: f32) -> Option<Bounds> {
        let field = self.field.as_ref()?;
        Some(self.on_screen(field.box_top, field.box_height, offset))
    }

    /// The error under the field: where its text starts and the text.
    pub fn field_error(&self, offset: f32) -> Option<(Bounds, &Arc<TextLayout>)> {
        let (top, text) = self.field.as_ref()?.error.as_ref()?;
        Some((self.on_screen(*top, text.height.ceil(), offset), text))
    }

    pub fn row(&self, index: usize, offset: f32) -> Option<Bounds> {
        (index < self.row_count).then(|| {
            self.on_screen(
                self.rows_top + index as f32 * self.row_height,
                self.row_height,
                offset,
            )
        })
    }

    /// What the point (`x`, `y`) lands on with the body scrolled by
    /// `offset`. Every region is half-open, so a pixel on the edge between
    /// two belongs to one.
    pub fn hit(&self, x: f32, y: f32, offset: f32) -> Option<DialogHit> {
        if let Some(button) = self.buttons.iter().position(|button| button.contains(x, y)) {
            return Some(DialogHit::Button(button));
        }
        if self.body.contains(x, y) {
            if self
                .field_box(offset)
                .is_some_and(|field| field.contains(x, y))
            {
                return Some(DialogHit::Field);
            }
            let first = self.body.y + self.rows_top - offset;
            if self.row_count > 0 && y >= first && x < self.body.x + self.content_width {
                let row = ((y - first) / self.row_height).floor() as usize;
                if self
                    .row(row, offset)
                    .is_some_and(|bounds| bounds.contains(x, y))
                {
                    return Some(DialogHit::Row(row));
                }
            }
            return Some(DialogHit::Body);
        }
        self.card.contains(x, y).then_some(DialogHit::Card)
    }

    /// The field's visible text and caret, for `text` with the caret at
    /// byte `cursor`: drawing, caret clicks, and the IME caret use it.
    pub fn field_viewport(
        &self,
        text: &str,
        cursor: usize,
        offset: f32,
    ) -> Option<TextInputViewport> {
        let field = self.field_box(offset)?;
        TextInputViewport::calculate(
            text,
            cursor,
            field.x + self.control_inset,
            (field.y + (field.height - self.cell_height) / 2.0).round(),
            field.right() - self.control_inset,
            self.cell_width,
        )
    }

    /// The caret position a press at `x` on the field puts in `text`.
    pub fn field_caret_at(&self, text: &str, cursor: usize, x: f32, offset: f32) -> Option<usize> {
        self.field_viewport(text, cursor, offset)?
            .cursor_at(text, x, self.cell_width)
    }
}

struct BodySizes<'a> {
    body: TextSize,
    value: TextSize,
    px: &'a dyn Fn(f32) -> f32,
}

fn paragraph(
    measure: &mut dyn TextMeasure,
    text: &str,
    face: UiFace,
    size: TextSize,
    width: f32,
) -> Arc<TextLayout> {
    measure.layout(&[TextSpan { text, face }], size, Some(width))
}

fn lay_out_body(
    dialog: &Dialog,
    width: f32,
    sizes: &BodySizes<'_>,
    measure: &mut dyn TextMeasure,
) -> Body {
    let px = sizes.px;
    let mut top = 0.0;
    let mut gap = 0.0;
    let mut place = |height: f32| {
        top += gap;
        let start = top;
        top += height;
        gap = px(BLOCK_GAP);
        start
    };
    let blocks = dialog
        .body()
        .iter()
        .map(|block| {
            let (text, inset) = match block {
                DialogBlock::Text(text) | DialogBlock::Error(text) => (
                    paragraph(measure, text, UiFace::SANS, sizes.body, width),
                    false,
                ),
                DialogBlock::Value(text) => {
                    let text_width = (width - px(VALUE_PADDING_X) * 2.0).max(1.0);
                    (
                        paragraph(measure, text, UiFace::MONO, sizes.value, text_width),
                        true,
                    )
                }
            };
            let (text_x, text_y) = if inset {
                (px(VALUE_PADDING_X), px(VALUE_PADDING_Y))
            } else {
                (0.0, 0.0)
            };
            let height = text.height.ceil() + text_y * 2.0;
            BlockLayout {
                top: place(height),
                height,
                text_x,
                text_y,
                text,
                inset,
            }
        })
        .collect();

    let field = dialog.field.as_ref().map(|field| {
        let label_height = sizes.body.line_px;
        let box_height = px(FIELD_HEIGHT);
        let error = field
            .error
            .as_deref()
            .map(|error| paragraph(measure, error, UiFace::SANS, sizes.body, width));
        let error_height = error
            .as_ref()
            .map_or(0.0, |error| px(FIELD_ERROR_GAP) + error.height.ceil());
        let label_top = place(label_height + px(FIELD_LABEL_GAP) + box_height + error_height);
        let box_top = label_top + label_height + px(FIELD_LABEL_GAP);
        FieldLayout {
            label_top,
            label_height,
            box_top,
            box_height,
            error: error.map(|error| (box_top + box_height + px(FIELD_ERROR_GAP), error)),
        }
    });

    let rows = dialog.rows().len();
    let rows_top = if rows > 0 {
        place(rows as f32 * px(ROW_HEIGHT))
    } else {
        top
    };
    Body {
        blocks,
        field,
        rows_top,
        height: top,
    }
}

/// The buttons, left to right, ending at `right`: each wide enough for its
/// label and at least the minimum, or all shrunk alike when they would not
/// fit in `room`.
fn lay_out_buttons(
    dialog: &Dialog,
    right: f32,
    top: f32,
    room: f32,
    sizes: &BodySizes<'_>,
    measure: &mut dyn TextMeasure,
) -> Vec<Bounds> {
    let px = sizes.px;
    let spacing = px(BUTTON_SPACING);
    let height = px(BUTTON_HEIGHT);
    let count = dialog.buttons().len();
    let mut widths: Vec<f32> = dialog
        .buttons()
        .iter()
        .map(|button| {
            let label = measure.width(&button.label, UiFace::SANS, sizes.body.font_px);
            (label + px(BUTTON_PADDING) * 2.0)
                .ceil()
                .max(px(BUTTON_MIN_WIDTH))
        })
        .collect();
    let gaps = spacing * count.saturating_sub(1) as f32;
    if widths.iter().sum::<f32>() + gaps > room {
        let shared = ((room - gaps) / count.max(1) as f32).floor().max(1.0);
        widths.fill(shared);
    }
    let mut x = right - widths.iter().sum::<f32>() - gaps;
    widths
        .into_iter()
        .map(|width| {
            let bounds = Bounds::new(x, top, width, height);
            x += width + spacing;
            bounds
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::{ButtonRole, DialogButton, DialogRow, DialogSpec, FieldSpec};
    use super::*;
    use crate::ui::text_layout::FixedMeasure;

    fn metrics(scale: f32) -> DialogMetrics {
        DialogMetrics {
            window_width: 1200.0 * scale,
            window_height: 800.0 * scale,
            scale,
            cell_width: 8.0 * scale,
            cell_height: 16.0 * scale,
            animation: 1.0,
        }
    }

    fn lay_out(spec: DialogSpec, metrics: DialogMetrics) -> (Dialog, DialogLayout) {
        let mut dialog = Dialog::new();
        dialog.show(spec);
        let layout = DialogLayout::calculate(
            &dialog,
            metrics,
            &mut FixedMeasure {
                advance: 7.0 * metrics.scale,
            },
        );
        (dialog, layout)
    }

    fn confirmation() -> DialogSpec {
        DialogSpec::confirm(
            "Delete workspace?",
            vec![
                super::super::DialogBlock::text("Its layout and saved scrollback are removed."),
                super::super::DialogBlock::value("~/.local/share/termeric/workspaces/roots.json"),
            ],
            "delete",
            "Delete",
            ButtonRole::Destructive,
        )
    }

    fn whole(bounds: Bounds) -> bool {
        [bounds.x, bounds.y, bounds.width, bounds.height]
            .iter()
            .all(|value| value.fract() == 0.0)
    }

    #[test]
    fn buttons_sit_right_aligned_on_whole_pixels_at_every_scale_and_phase() {
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            for animation in [0.0, 0.3, 0.75, 1.0] {
                let (_, layout) = lay_out(
                    confirmation(),
                    DialogMetrics {
                        animation,
                        ..metrics(scale)
                    },
                );
                let px = |logical: f32| (logical * scale).round();
                assert_eq!(layout.card.width, px(440.0));
                assert!(whole(layout.card) && whole(layout.body) && whole(layout.heading_bounds));
                let [cancel, delete] = layout.buttons[..] else {
                    panic!("two buttons");
                };
                assert_eq!(delete.right(), layout.card.right() - px(20.0));
                assert_eq!(delete.x - cancel.right(), px(8.0));
                for button in [cancel, delete] {
                    assert!(whole(button), "{scale} {animation}: {button:?}");
                    assert_eq!(button.height, px(32.0));
                    assert!(button.width >= px(88.0));
                    assert_eq!(button.bottom(), layout.card.bottom() - px(20.0));
                }
                assert_eq!(cancel.y - layout.body.bottom(), px(20.0));
                assert_eq!(layout.body.y - layout.heading_bounds.bottom(), px(8.0));
                assert_eq!(layout.heading_bounds.y, layout.card.y + px(20.0));
                assert_eq!(layout.heading_bounds.x, layout.card.x + px(20.0));
            }
        }
    }

    #[test]
    fn widths_follow_the_shape_and_never_leave_the_window() {
        let (_, prompt) = lay_out(
            DialogSpec::prompt(
                "Rename?",
                vec![],
                FieldSpec::new("Name", "", 32),
                "save",
                "Save",
            ),
            metrics(1.0),
        );
        assert_eq!(prompt.card.width, 560.0);
        let (_, narrow) = lay_out(
            confirmation(),
            DialogMetrics {
                window_width: 220.0,
                ..metrics(1.0)
            },
        );
        assert_eq!(narrow.card.width, 220.0 - 48.0);
        assert!(narrow.card.x >= 24.0 && narrow.card.right() <= 220.0 - 24.0);
        // Buttons too wide for the row share it alike.
        assert_eq!(narrow.buttons[0].x, narrow.card.x + 20.0);
        assert_eq!(narrow.buttons[1].right(), narrow.card.right() - 20.0);
        assert_eq!(narrow.buttons[0].width, narrow.buttons[1].width);

        // A longer label widens its button past the minimum.
        let (_, wide) = lay_out(
            DialogSpec::confirm(
                "?",
                vec![],
                "go",
                "Close 3 tabs and open roots",
                ButtonRole::Suggested,
            ),
            metrics(1.0),
        );
        assert!(wide.buttons[1].width > 88.0);
        assert_eq!(wide.buttons[0].width, 88.0);
    }

    #[test]
    fn an_empty_body_takes_no_room_and_blocks_stack_eight_apart() {
        let (_, empty) = lay_out(DialogSpec::message("Saved", vec![]), metrics(1.0));
        assert_eq!(empty.body.height, 0.0);
        assert_eq!(empty.buttons[0].y - empty.heading_bounds.bottom(), 20.0);

        let (_, layout) = lay_out(confirmation(), metrics(1.0));
        let [text, value] = &layout.blocks[..] else {
            panic!("two blocks");
        };
        assert_eq!(text.top, 0.0);
        assert_eq!(value.top, text.top + text.height + 8.0);
        assert!(value.inset && !text.inset);
        assert_eq!(layout.content_height, value.top + value.height);
        assert_eq!(layout.body.height, layout.content_height, "it all fits");
        assert!(layout.scrollbar.is_none());
    }

    #[test]
    fn only_the_body_scrolls_and_hits_follow_what_is_drawn() {
        let spec = DialogSpec::list(
            "Pick a plugin",
            vec![super::super::DialogBlock::text("Installed plugins.")],
            (0..60)
                .map(|index| DialogRow::new(format!("row:{index}"), format!("Row {index}")))
                .collect(),
        )
        .button(DialogButton::cancel());
        let (_, layout) = lay_out(spec, metrics(1.0));
        assert!(layout.content_height > layout.body.height);
        let track = layout.scrollbar.expect("the body scrolls");
        assert_eq!(track.right(), layout.body.right());
        assert!(layout.body.x + layout.content_width <= track.x);
        assert!(layout.card.bottom() <= 800.0 - 24.0);

        let button = layout.buttons[0];
        for offset in [0.0, 37.0, layout.content_height - layout.body.height] {
            // Heading and buttons stay put however far the body scrolls.
            let [x, y] = button.center();
            assert_eq!(layout.hit(x, y, offset), Some(DialogHit::Button(0)));
            let heading = layout.heading_bounds.center();
            assert_eq!(
                layout.hit(heading[0], heading[1], offset),
                Some(DialogHit::Card)
            );
            // A row answers exactly where it is drawn, and only inside the
            // viewport.
            for row in 0..60 {
                let bounds = layout.row(row, offset).unwrap();
                let x = bounds.x + 1.0;
                match layout.visible(bounds) {
                    Some(visible) => {
                        assert_eq!(layout.hit(x, visible.y, offset), Some(DialogHit::Row(row)));
                        assert_eq!(
                            layout.hit(x, visible.bottom() - 0.01, offset),
                            Some(DialogHit::Row(row))
                        );
                    }
                    None => assert_ne!(
                        layout.hit(x, bounds.center()[1], offset),
                        Some(DialogHit::Row(row))
                    ),
                }
            }
        }
        // The pixel between two rows belongs to the lower one.
        let second = layout.row(1, 0.0).unwrap();
        assert_eq!(
            layout.hit(second.x + 1.0, second.y, 0.0),
            Some(DialogHit::Row(1))
        );
        // Half-open buttons: the right edge is outside.
        assert_ne!(
            layout.hit(button.right(), button.y + 1.0, 0.0),
            Some(DialogHit::Button(0))
        );
        assert_eq!(
            layout.hit(button.x, button.y, 0.0),
            Some(DialogHit::Button(0))
        );
        assert_eq!(layout.hit(layout.card.x - 1.0, layout.card.y, 0.0), None);
    }

    #[test]
    fn the_field_sits_under_its_label_and_maps_presses_to_the_caret() {
        let spec = DialogSpec::prompt(
            "Add feature",
            vec![super::super::DialogBlock::text("tag = value")],
            FieldSpec::new("Feature", "ss01 = 1", 32).with_error(Some("expected tag = value")),
            "save",
            "Add",
        );
        let (dialog, layout) = lay_out(spec, metrics(1.0));
        let field = layout.field.as_ref().unwrap();
        let label = layout.field_label(0.0).unwrap();
        let input = layout.field_box(0.0).unwrap();
        assert_eq!(input.y - label.bottom(), 6.0);
        assert_eq!(input.height, 32.0);
        let (error, _) = layout.field_error(0.0).unwrap();
        assert_eq!(error.y - input.bottom(), 6.0);
        assert_eq!(
            layout.span(DialogFocus::Field),
            Some((field.label_top, input.bottom() - layout.body.y))
        );
        let [x, y] = input.center();
        assert_eq!(layout.hit(x, y, 0.0), Some(DialogHit::Field));
        assert_eq!(layout.hit(x, label.center()[1], 0.0), Some(DialogHit::Body));

        let value = dialog.field(dialog.request_id()).unwrap().value;
        let view = layout.field_viewport(&value, 0, 0.0).unwrap();
        assert_eq!(view.text_x, input.x + 10.0);
        assert_eq!(view.text_y.fract(), 0.0);
        // A press on the third cell puts the caret before it.
        let third = view.text_x + 8.0 * 2.0 + 1.0;
        assert_eq!(layout.field_caret_at(&value, 0, third, 0.0), Some(2));
    }
}
