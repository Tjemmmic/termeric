//! Small vector icons for chrome controls, drawn with line and rounded-rect
//! primitives so they stay crisp at every scale and follow the theme.

use super::Renderer;

/// Line icons drawn centred on a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Icon {
    Close,
    Plus,
    Minus,
    Back,
    ChevronRight,
    ChevronUp,
    ChevronDown,
    /// Three narrowing lines: show only matching lines.
    Filter,
}

impl Renderer {
    /// Draw `icon` centred on (`cx`, `cy`).
    pub(super) fn push_icon(&mut self, icon: Icon, cx: f32, cy: f32, color: [f32; 4]) {
        let scale = self.scale_factor;
        let stroke = (1.5 * scale).max(1.0);
        let arm = 4.0 * scale;
        let lines: &[(f32, f32, f32, f32)] = match icon {
            Icon::Close => &[(-arm, -arm, arm, arm), (arm, -arm, -arm, arm)],
            Icon::Plus => &[(-arm, 0.0, arm, 0.0), (0.0, -arm, 0.0, arm)],
            Icon::Minus => &[(-arm, 0.0, arm, 0.0)],
            Icon::Back => &[
                (arm * 0.5, -arm, -arm * 0.5, 0.0),
                (-arm * 0.5, 0.0, arm * 0.5, arm),
            ],
            Icon::ChevronRight => &[
                (-arm * 0.5, -arm, arm * 0.5, 0.0),
                (arm * 0.5, 0.0, -arm * 0.5, arm),
            ],
            Icon::ChevronUp => &[
                (-arm, arm * 0.5, 0.0, -arm * 0.5),
                (0.0, -arm * 0.5, arm, arm * 0.5),
            ],
            Icon::ChevronDown => &[
                (-arm, -arm * 0.5, 0.0, arm * 0.5),
                (0.0, arm * 0.5, arm, -arm * 0.5),
            ],
            Icon::Filter => &[
                (-arm * 1.1, -arm * 0.7, arm * 1.1, -arm * 0.7),
                (-arm * 0.7, 0.0, arm * 0.7, 0.0),
                (-arm * 0.3, arm * 0.7, arm * 0.3, arm * 0.7),
            ],
        };
        for &(x1, y1, x2, y2) in lines {
            self.rect_pipeline
                .push_line(cx + x1, cy + y1, cx + x2, cy + y2, stroke, color);
        }
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
        let px = |logical: f32| (logical * self.scale_factor).round().max(1.0);
        let (cx, cy) = (cx.round(), cy.round());
        let ring = px(10.0);
        self.rect_pipeline.push_rounded_rect(
            cx - ring / 2.0 - px(1.0),
            cy - ring / 2.0 - px(1.0),
            ring,
            ring,
            color,
            ring / 2.0,
        );
        let hole = ring - px(3.0);
        self.rect_pipeline.push_rounded_rect(
            cx - hole / 2.0 - px(1.0),
            cy - hole / 2.0 - px(1.0),
            hole,
            hole,
            background,
            hole / 2.0,
        );
        self.rect_pipeline.push_line(
            cx + px(2.0),
            cy + px(2.0),
            cx + px(5.0),
            cy + px(5.0),
            (1.6 * self.scale_factor).max(1.0),
            color,
        );
    }
}
