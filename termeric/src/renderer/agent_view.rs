//! Drawing the agent view panel from its scene.
//!
//! The panel's layout, hit targets, and accessibility come from
//! `crate::ui::agent_view`; this module only turns scene primitives into
//! rectangles, strokes, and text, clipped to their layers.

use std::f32::consts::TAU;
use std::sync::Arc;

use super::Renderer;
use super::text::TextClip;
use crate::ui::agent_view::scene::{Bounds, Icon, Layer, Prim, Scene};

/// Segments used for a full circle; arcs use a share of them.
const CIRCLE_SEGMENTS: usize = 28;

/// One stroke of an icon, in a 16-unit box centred on the origin.
enum Stroke {
    Path(&'static [[f32; 2]]),
    Circle {
        center: [f32; 2],
        radius: f32,
    },
    Dot {
        center: [f32; 2],
        radius: f32,
    },
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        radius: f32,
        filled: bool,
    },
}

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
    }

    fn draw_layer(&mut self, layer: &Layer, background: [f32; 4]) {
        let clip = layer.clip.map(text_clip);
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
            } => self.push_scene_icon(*icon, *center, *size, opaque(*color, background)),
            Prim::Spinner {
                center,
                radius,
                thickness,
                phase,
                color,
            } => {
                let start = phase * TAU;
                self.push_arc(
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
                    self.push_arc(
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

    fn push_arc(
        &mut self,
        center: [f32; 2],
        radius: f32,
        thickness: f32,
        start: f32,
        end: f32,
        color: [f32; 4],
    ) {
        let segments = (((end - start) / TAU) * CIRCLE_SEGMENTS as f32)
            .ceil()
            .max(1.0) as usize;
        let point = |angle: f32| {
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        };
        let mut previous = point(start);
        for step in 1..=segments {
            let angle = start + (end - start) * step as f32 / segments as f32;
            let next = point(angle);
            self.rect_pipeline.push_line(
                previous[0],
                previous[1],
                next[0],
                next[1],
                thickness,
                color,
            );
            previous = next;
        }
    }

    fn push_scene_icon(&mut self, icon: Icon, center: [f32; 2], size: f32, color: [f32; 4]) {
        let unit = size / 16.0;
        let stroke = (1.5 * unit).max(1.0);
        let at = |point: [f32; 2]| [center[0] + point[0] * unit, center[1] + point[1] * unit];
        for part in strokes(icon) {
            match part {
                Stroke::Path(points) => {
                    for pair in points.windows(2) {
                        let (a, b) = (at(pair[0]), at(pair[1]));
                        self.rect_pipeline
                            .push_line(a[0], a[1], b[0], b[1], stroke, color);
                    }
                }
                Stroke::Circle {
                    center: middle,
                    radius,
                } => {
                    self.push_arc(at(middle), radius * unit, stroke, 0.0, TAU, color);
                }
                Stroke::Dot {
                    center: middle,
                    radius,
                } => {
                    let middle = at(middle);
                    let radius = radius * unit;
                    self.rect_pipeline.push_rounded_rect(
                        middle[0] - radius,
                        middle[1] - radius,
                        radius * 2.0,
                        radius * 2.0,
                        color,
                        radius,
                    );
                }
                Stroke::Rect {
                    x,
                    y,
                    width,
                    height,
                    radius,
                    filled,
                } => {
                    let origin = at([x, y]);
                    if filled {
                        self.rect_pipeline.push_rounded_rect(
                            origin[0],
                            origin[1],
                            width * unit,
                            height * unit,
                            color,
                            radius * unit,
                        );
                    } else {
                        self.rect_pipeline.push_rounded_outline(
                            origin[0] - stroke / 2.0,
                            origin[1] - stroke / 2.0,
                            width * unit + stroke,
                            height * unit + stroke,
                            color,
                            radius * unit + stroke / 2.0,
                            stroke,
                        );
                    }
                }
            }
        }
    }
}

fn text_clip(bounds: Bounds) -> TextClip {
    TextClip {
        left: bounds.x,
        top: bounds.y,
        right: bounds.right(),
        bottom: bounds.bottom(),
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

fn strokes(icon: Icon) -> Vec<Stroke> {
    use Stroke::{Circle, Dot, Path, Rect};
    match icon {
        Icon::Plus => vec![
            Path(&[[-5.0, 0.0], [5.0, 0.0]]),
            Path(&[[0.0, -5.0], [0.0, 5.0]]),
        ],
        Icon::Close | Icon::Cross => vec![
            Path(&[[-4.5, -4.5], [4.5, 4.5]]),
            Path(&[[4.5, -4.5], [-4.5, 4.5]]),
        ],
        Icon::ChevronDown => vec![Path(&[[-4.0, -2.0], [0.0, 2.0], [4.0, -2.0]])],
        Icon::ChevronUp => vec![Path(&[[-4.0, 2.0], [0.0, -2.0], [4.0, 2.0]])],
        Icon::ChevronRight => vec![Path(&[[-2.0, -4.0], [2.0, 0.0], [-2.0, 4.0]])],
        Icon::Back => vec![Path(&[[2.0, -4.5], [-2.5, 0.0], [2.0, 4.5]])],
        Icon::History => vec![
            Circle {
                center: [0.0, 0.0],
                radius: 5.8,
            },
            Path(&[[0.0, -3.2], [0.0, 0.2], [2.2, 1.6]]),
        ],
        Icon::More => vec![
            Dot {
                center: [-4.5, 0.0],
                radius: 1.3,
            },
            Dot {
                center: [0.0, 0.0],
                radius: 1.3,
            },
            Dot {
                center: [4.5, 0.0],
                radius: 1.3,
            },
        ],
        Icon::Send => vec![
            Path(&[[0.0, 5.5], [0.0, -5.0]]),
            Path(&[[-4.5, -0.5], [0.0, -5.0], [4.5, -0.5]]),
        ],
        Icon::Stop => vec![Rect {
            x: -4.0,
            y: -4.0,
            width: 8.0,
            height: 8.0,
            radius: 1.5,
            filled: true,
        }],
        Icon::Terminal => vec![
            Rect {
                x: -6.5,
                y: -5.0,
                width: 13.0,
                height: 10.0,
                radius: 2.0,
                filled: false,
            },
            Path(&[[-3.5, -1.8], [-1.2, 0.0], [-3.5, 1.8]]),
            Path(&[[0.5, 2.2], [3.5, 2.2]]),
        ],
        Icon::File => vec![
            Path(&[
                [-4.5, -6.5],
                [1.0, -6.5],
                [4.5, -3.0],
                [4.5, 6.5],
                [-4.5, 6.5],
                [-4.5, -6.5],
            ]),
            Path(&[[1.0, -6.5], [1.0, -3.0], [4.5, -3.0]]),
        ],
        Icon::Pencil => vec![
            Path(&[
                [2.8, -5.5],
                [5.5, -2.8],
                [-2.0, 4.7],
                [-5.0, 5.0],
                [-4.7, 2.0],
                [2.8, -5.5],
            ]),
            Path(&[[0.8, -3.5], [3.5, -0.8]]),
        ],
        Icon::Search => vec![
            Circle {
                center: [-1.2, -1.2],
                radius: 4.3,
            },
            Path(&[[2.0, 2.0], [5.5, 5.5]]),
        ],
        Icon::Shield => vec![Path(&[
            [0.0, -6.5],
            [5.5, -4.5],
            [5.5, 0.0],
            [4.0, 3.5],
            [0.0, 6.5],
            [-4.0, 3.5],
            [-5.5, 0.0],
            [-5.5, -4.5],
            [0.0, -6.5],
        ])],
        Icon::Check => vec![Path(&[[-4.5, 0.3], [-1.5, 3.5], [4.5, -3.5]])],
        Icon::Eye => vec![
            Path(&[
                [-7.0, 0.0],
                [-4.5, -3.3],
                [0.0, -4.8],
                [4.5, -3.3],
                [7.0, 0.0],
                [4.5, 3.3],
                [0.0, 4.8],
                [-4.5, 3.3],
                [-7.0, 0.0],
            ]),
            Circle {
                center: [0.0, 0.0],
                radius: 2.0,
            },
        ],
        Icon::Plan => vec![
            Path(&[[-5.5, -4.0], [5.5, -4.0]]),
            Path(&[[-5.5, 0.0], [5.5, 0.0]]),
            Path(&[[-5.5, 4.0], [1.0, 4.0]]),
        ],
        Icon::Copy => vec![
            Rect {
                x: -5.5,
                y: -3.0,
                width: 8.0,
                height: 9.0,
                radius: 1.5,
                filled: false,
            },
            Path(&[
                [-2.5, -3.0],
                [-2.5, -6.0],
                [5.5, -6.0],
                [5.5, 3.0],
                [2.5, 3.0],
            ]),
        ],
        Icon::Globe => vec![
            Circle {
                center: [0.0, 0.0],
                radius: 6.0,
            },
            Path(&[[-6.0, 0.0], [6.0, 0.0]]),
            Path(&[
                [0.0, -6.0],
                [-2.5, -3.0],
                [-3.0, 0.0],
                [-2.5, 3.0],
                [0.0, 6.0],
            ]),
            Path(&[[0.0, -6.0], [2.5, -3.0], [3.0, 0.0], [2.5, 3.0], [0.0, 6.0]]),
        ],
        Icon::Agent => vec![
            Rect {
                x: -5.5,
                y: -4.5,
                width: 11.0,
                height: 10.0,
                radius: 3.0,
                filled: false,
            },
            Dot {
                center: [-2.2, 0.0],
                radius: 1.1,
            },
            Dot {
                center: [2.2, 0.0],
                radius: 1.1,
            },
            Path(&[[0.0, -4.5], [0.0, -6.5]]),
        ],
        Icon::Folder => vec![Path(&[
            [-6.5, -4.5],
            [-2.5, -4.5],
            [-1.0, -3.0],
            [6.5, -3.0],
            [6.5, 5.0],
            [-6.5, 5.0],
            [-6.5, -4.5],
        ])],
        Icon::Warning => vec![
            Path(&[[0.0, -6.0], [6.5, 5.5], [-6.5, 5.5], [0.0, -6.0]]),
            Path(&[[0.0, -2.0], [0.0, 1.5]]),
            Dot {
                center: [0.0, 3.5],
                radius: 0.9,
            },
        ],
    }
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

    #[test]
    fn every_icon_has_strokes() {
        for icon in [
            Icon::Plus,
            Icon::Close,
            Icon::ChevronDown,
            Icon::ChevronRight,
            Icon::ChevronUp,
            Icon::Back,
            Icon::History,
            Icon::More,
            Icon::Send,
            Icon::Stop,
            Icon::Terminal,
            Icon::File,
            Icon::Pencil,
            Icon::Search,
            Icon::Shield,
            Icon::Check,
            Icon::Cross,
            Icon::Eye,
            Icon::Plan,
            Icon::Copy,
            Icon::Globe,
            Icon::Agent,
            Icon::Folder,
            Icon::Warning,
        ] {
            assert!(!strokes(icon).is_empty());
        }
    }
}
