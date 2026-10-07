//! Drawing the agent view panel from its scene.
//!
//! The panel's layout, hit targets, and accessibility come from
//! `crate::ui::agent_view`; this module only turns scene primitives into
//! rectangles, strokes, and text, clipped to their layers.

use std::f32::consts::TAU;
use std::sync::Arc;

use super::Renderer;
use super::icons::{draw_arc, draw_icon};
use super::text::TextClip;
use crate::ui::agent_view::scene::{Layer, Prim, Scene};

impl Renderer {
    /// Shows the panel's scene, or nothing.
    pub fn set_agent_view(&mut self, scene: Option<Arc<Scene>>) {
        self.agent_view = scene;
    }

    /// Width reserved on the right for the docked panel, in physical pixels.
    /// Returns whether it changed; terminal geometry must then be synchronized.
    pub fn set_dock_width(&mut self, width: f32) -> bool {
        let width = width.max(0.0).round();
        if (width - self.dock_width).abs() < 0.5 {
            return false;
        }
        self.dock_width = width;
        true
    }

    pub fn dock_width(&self) -> f32 {
        self.dock_width
    }

    /// The right edge of the terminal area for a viewport width.
    pub fn content_right_for(&self, window_width: f32) -> f32 {
        (window_width - self.dock_width).max(self.tab_bar_width_for(window_width))
    }

    /// The right edge of the terminal area.
    pub fn content_right(&self) -> f32 {
        self.content_right_for(self.window_size().0 as f32)
    }

    /// Width of the terminal area between the tab rail and the dock.
    pub fn content_width_for(&self, window_width: f32) -> f32 {
        (self.content_right_for(window_width) - self.tab_bar_width_for(window_width)).max(0.0)
    }

    pub(super) fn build_agent_view(&mut self) {
        let Some(scene) = self.agent_view.clone() else {
            return;
        };
        self.draw_layer(&scene.base, scene.background);
        self.draw_layer(&scene.feed, scene.background);
    }

    pub(super) fn build_agent_view_overlay(&mut self) {
        let Some(scene) = self.agent_view.clone() else {
            return;
        };
        self.draw_layer(&scene.overlay, scene.background);
        self.draw_layer(&scene.menu, scene.background);
    }

    fn draw_layer(&mut self, layer: &Layer, background: [f32; 4]) {
        let clip = layer.clip.map(TextClip::from);
        let previous = self.rect_pipeline.set_clip(clip);
        for prim in &layer.prims {
            self.draw_prim(prim, clip, background);
        }
        self.rect_pipeline.set_clip(previous);
    }

    fn draw_prim(&mut self, prim: &Prim, clip: Option<TextClip>, background: [f32; 4]) {
        match prim {
            Prim::Fill {
                bounds,
                radius,
                color,
            } => {
                self.rect_pipeline.push_rounded_rect(
                    bounds.x,
                    bounds.y,
                    bounds.width,
                    bounds.height,
                    *color,
                    *radius,
                );
            }
            Prim::Outline {
                bounds,
                radius,
                stroke,
                color,
            } => self.rect_pipeline.push_rounded_outline(
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                *color,
                *radius,
                *stroke,
            ),
            Prim::Paragraph {
                layout,
                x,
                y,
                font_px,
                paint,
            } => {
                let paint = Arc::clone(paint);
                self.push_text_layout(
                    layout,
                    *x,
                    *y,
                    *font_px,
                    |span| {
                        paint
                            .get(span)
                            .or(paint.last())
                            .map_or((Default::default(), [1.0; 4]), |paint| {
                                (paint.face, paint.color)
                            })
                    },
                    clip,
                );
            }
            Prim::Label {
                text,
                x,
                baseline,
                font_px,
                face,
                color,
            } => self
                .text_pipeline
                .push_ui_text_in(text, *face, *x, *baseline, *font_px, *color, clip),
            Prim::Icon {
                icon,
                center,
                size,
                color,
            } => draw_icon(
                &mut self.rect_pipeline,
                *icon,
                *center,
                *size,
                opaque(*color, background),
            ),
            Prim::Spinner {
                center,
                radius,
                thickness,
                phase,
                color,
            } => {
                let start = phase * TAU;
                draw_arc(
                    &mut self.rect_pipeline,
                    *center,
                    *radius,
                    *thickness,
                    start,
                    start + TAU * 0.72,
                    opaque(*color, background),
                );
            }
            Prim::Arc {
                center,
                radius,
                thickness,
                fraction,
                color,
            } => {
                let start = -TAU / 4.0;
                let sweep = TAU * fraction.clamp(0.0, 1.0);
                if sweep > 0.001 {
                    draw_arc(
                        &mut self.rect_pipeline,
                        *center,
                        *radius,
                        *thickness,
                        start,
                        start + sweep,
                        opaque(*color, background),
                    );
                }
            }
        }
    }
}

/// Blends a translucent stroke color over the panel, so joints between a
/// path's segments do not show as darker dots.
fn opaque(color: [f32; 4], background: [f32; 4]) -> [f32; 4] {
    let alpha = color[3];
    if alpha >= 1.0 || background[3] < 1.0 {
        return color;
    }
    [
        color[0] * alpha + background[0] * (1.0 - alpha),
        color[1] * alpha + background[1] * (1.0 - alpha),
        color[2] * alpha + background[2] * (1.0 - alpha),
        1.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translucent_strokes_become_opaque_over_the_panel() {
        let blended = opaque([1.0, 1.0, 1.0, 0.5], [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(blended, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(opaque([0.2, 0.3, 0.4, 1.0], [0.0; 4]), [0.2, 0.3, 0.4, 1.0]);
    }
}
