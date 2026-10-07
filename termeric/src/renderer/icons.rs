//! Vector icons for chrome controls and the agent view: one stroke table,
//! drawn with line and rounded-rect primitives so icons stay crisp at every
//! scale and follow the theme.

use std::f32::consts::TAU;

use super::Renderer;
use super::rect::RectPipeline;
use crate::ui::icon::Icon;

/// Logical size of a chrome icon's 16-unit box.
const CHROME_ICON_SIZE: f32 = 16.0;

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
    /// Draw `icon` at chrome size, centred on (`cx`, `cy`).
    pub(super) fn push_icon(&mut self, icon: Icon, cx: f32, cy: f32, color: [f32; 4]) {
        draw_icon(
            &mut self.rect_pipeline,
            icon,
            [cx, cy],
            CHROME_ICON_SIZE * self.scale_factor,
            color,
        );
    }

    /// A magnifying glass centred on (`cx`, `cy`). `background` fills the
    /// lens so the ring reads on any surface.
    pub(super) fn push_magnifier(
        &mut self,
        cx: f32,
        cy: f32,
        color: [f32; 4],
        background: [f32; 4],
    ) {
        let (cx, cy) = (cx.round(), cy.round());
        let ring = self.px(10.0);
        self.rect_pipeline.push_rounded_rect(
            cx - ring / 2.0 - self.px(1.0),
            cy - ring / 2.0 - self.px(1.0),
            ring,
            ring,
            color,
            ring / 2.0,
        );
        let hole = ring - self.px(3.0);
        self.rect_pipeline.push_rounded_rect(
            cx - hole / 2.0 - self.px(1.0),
            cy - hole / 2.0 - self.px(1.0),
            hole,
            hole,
            background,
            hole / 2.0,
        );
        self.rect_pipeline.push_line(
            cx + self.px(2.0),
            cy + self.px(2.0),
            cx + self.px(5.0),
            cy + self.px(5.0),
            (1.6 * self.scale_factor).max(1.0),
            color,
        );
    }
}

/// Draw `icon` with its 16-unit box scaled to `size` pixels, centred on
/// `center`.
pub(super) fn draw_icon(
    rect: &mut RectPipeline,
    icon: Icon,
    center: [f32; 2],
    size: f32,
    color: [f32; 4],
) {
    let unit = size / 16.0;
    let stroke = (1.5 * unit).max(1.0);
    let at = |point: [f32; 2]| [center[0] + point[0] * unit, center[1] + point[1] * unit];
    for part in strokes(icon) {
        match part {
            Stroke::Path(points) => {
                for pair in points.windows(2) {
                    let (a, b) = (at(pair[0]), at(pair[1]));
                    rect.push_line(a[0], a[1], b[0], b[1], stroke, color);
                }
            }
            Stroke::Circle {
                center: middle,
                radius,
            } => {
                draw_arc(rect, at(middle), radius * unit, stroke, 0.0, TAU, color);
            }
            Stroke::Dot {
                center: middle,
                radius,
            } => {
                let middle = at(middle);
                let radius = radius * unit;
                rect.push_rounded_rect(
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
                    rect.push_rounded_rect(
                        origin[0],
                        origin[1],
                        width * unit,
                        height * unit,
                        color,
                        radius * unit,
                    );
                } else {
                    rect.push_rounded_outline(
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

/// An arc of `radius` around `center` from angle `start` to `end`.
pub(super) fn draw_arc(
    rect: &mut RectPipeline,
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
        rect.push_line(previous[0], previous[1], next[0], next[1], thickness, color);
        previous = next;
    }
}

fn strokes(icon: Icon) -> Vec<Stroke> {
    use Stroke::{Circle, Dot, Path, Rect};
    match icon {
        Icon::Plus => vec![
            Path(&[[-5.0, 0.0], [5.0, 0.0]]),
            Path(&[[0.0, -5.0], [0.0, 5.0]]),
        ],
        Icon::Minus => vec![Path(&[[-5.0, 0.0], [5.0, 0.0]])],
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
        Icon::Prompt => vec![
            Path(&[[-6.0, -4.5], [-1.5, 0.0], [-6.0, 4.5]]),
            Path(&[[1.0, 4.5], [6.5, 4.5]]),
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
        Icon::Filter => vec![
            Path(&[[-4.4, -2.8], [4.4, -2.8]]),
            Path(&[[-2.8, 0.0], [2.8, 0.0]]),
            Path(&[[-1.2, 2.8], [1.2, 2.8]]),
        ],
        Icon::Menu => vec![
            Path(&[[-6.5, -4.5], [6.5, -4.5]]),
            Path(&[[-6.5, 0.0], [6.5, 0.0]]),
            Path(&[[-6.5, 4.5], [6.5, 4.5]]),
        ],
        Icon::Branch => vec![
            Path(&[[0.0, -5.0], [0.0, 5.0]]),
            Path(&[[3.5, -4.0], [0.0, -0.5]]),
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
        // Four corners pointing out: fill the space.
        Icon::Expand => vec![
            Path(&[[-6.0, -2.0], [-6.0, -6.0], [-2.0, -6.0]]),
            Path(&[[2.0, -6.0], [6.0, -6.0], [6.0, -2.0]]),
            Path(&[[6.0, 2.0], [6.0, 6.0], [2.0, 6.0]]),
            Path(&[[-2.0, 6.0], [-6.0, 6.0], [-6.0, 2.0]]),
        ],
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
    fn every_icon_has_strokes() {
        for icon in [
            Icon::Plus,
            Icon::Minus,
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
            Icon::Prompt,
            Icon::File,
            Icon::Pencil,
            Icon::Search,
            Icon::Filter,
            Icon::Menu,
            Icon::Branch,
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
            Icon::Expand,
        ] {
            assert!(!strokes(icon).is_empty());
        }
    }
}
