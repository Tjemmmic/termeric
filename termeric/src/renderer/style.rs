//! Resolution of terminal SGR state into one renderer-facing style.

use super::TerminalPalette;
use super::glyph_cache::FontStyle;
use super::types::{TextBlinkPhases, VisibleTextBlink};
use crate::ui::color::scale_rgb;
use termeric_core::{Cell, CellAttrs, Color as TermColor};

const DIM_FACTOR: f32 = 0.66;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UnderlineStyle {
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ResolvedCellStyle {
    pub foreground: TermColor,
    pub background: TermColor,
    pub underline_color: TermColor,
    pub font: FontStyle,
    pub underline: Option<UnderlineStyle>,
    pub strikethrough: bool,
    pub paint_foreground: bool,
    dim: bool,
}

impl ResolvedCellStyle {
    pub(super) fn resolve(
        palette: &TerminalPalette,
        dynamic: &termeric_core::DynamicColors,
        cell: &Cell,
        bold_is_bright: bool,
        blink: TextBlinkPhases,
    ) -> Self {
        let mut foreground =
            palette.resolve(dynamic, cell.fg_spec, cell.fg, palette.default_fg(dynamic));
        if bold_is_bright
            && cell.attrs.contains(CellAttrs::BOLD)
            && let termeric_core::grid::ColorSpec::Indexed(index @ 0..=7) = cell.fg_spec
        {
            foreground = dynamic
                .palette(index + 8)
                .unwrap_or(palette.ansi[index as usize + 8]);
        }
        let mut background =
            palette.resolve(dynamic, cell.bg_spec, cell.bg, palette.default_bg(dynamic));
        if cell.attrs.contains(CellAttrs::INVERSE) {
            std::mem::swap(&mut foreground, &mut background);
        }

        let blink_visible = if cell.attrs.contains(CellAttrs::RAPID_BLINK) {
            blink.rapid_visible
        } else if cell.attrs.contains(CellAttrs::BLINK) {
            blink.slow_visible
        } else {
            true
        };
        let paint_foreground = !cell.attrs.contains(CellAttrs::HIDDEN) && blink_visible;

        Self {
            foreground,
            background,
            underline_color: cell.underline_color.unwrap_or(foreground),
            font: FontStyle {
                bold: cell.attrs.contains(CellAttrs::BOLD),
                italic: cell.attrs.contains(CellAttrs::ITALIC),
            },
            underline: resolve_underline(cell.attrs),
            strikethrough: cell.attrs.contains(CellAttrs::STRIKE),
            paint_foreground,
            dim: cell.attrs.contains(CellAttrs::DIM),
        }
    }

    /// Resolve terminal foreground, selection/cursor override, dim, and pane
    /// focus brightness in that order.
    pub(super) fn text_color(self, override_color: Option<[f32; 4]>, brightness: f32) -> [f32; 4] {
        let color = override_color.unwrap_or_else(|| self.foreground.to_f32_array());
        shade_foreground(color, self.dim, brightness)
    }

    pub(super) fn decoration_color(self, brightness: f32) -> [f32; 4] {
        shade_foreground(self.underline_color.to_f32_array(), self.dim, brightness)
    }
}

impl super::Renderer {
    pub(super) fn draw_sgr_decorations(
        &mut self,
        style: ResolvedCellStyle,
        x: f32,
        y: f32,
        width: f32,
        brightness: f32,
        override_color: Option<([f32; 4], f32)>,
    ) {
        queue_sgr_decorations(
            &mut self.rect_pipeline,
            style,
            x,
            y,
            width,
            self.cell_height,
            brightness,
            override_color,
        );
    }
}

pub(super) fn queue_sgr_decorations(
    rects: &mut super::rect::RectPipeline,
    style: ResolvedCellStyle,
    x: f32,
    y: f32,
    width: f32,
    cell_height: f32,
    brightness: f32,
    override_color: Option<([f32; 4], f32)>,
) {
    let underline_color = override_color.map_or_else(
        || style.decoration_color(brightness),
        |(color, alpha)| {
            let mut resolved = style.text_color(Some(color), brightness);
            resolved[3] = alpha;
            resolved
        },
    );
    let underline_y = y + cell_height - 2.0;
    match style.underline {
        Some(UnderlineStyle::Curly) => {
            let segment_width = 1.5_f32;
            let mut current_x = x;
            let mut phase = 0.0_f32;
            while current_x < x + width {
                let offset_y = (phase * std::f32::consts::TAU).sin() * 2.0;
                rects.push_rect(
                    current_x,
                    underline_y + offset_y - 2.0,
                    segment_width.min(x + width - current_x),
                    1.5,
                    underline_color,
                );
                current_x += segment_width;
                phase += segment_width / 6.0;
            }
        }
        Some(UnderlineStyle::Double) => {
            rects.push_rect(x, underline_y - 2.0, width, 1.0, underline_color);
            rects.push_rect(x, underline_y, width, 1.0, underline_color);
        }
        Some(UnderlineStyle::Dotted) => {
            let mut current_x = x;
            while current_x < x + width {
                rects.push_rect(
                    current_x,
                    underline_y,
                    1.5_f32.min(x + width - current_x),
                    1.0,
                    underline_color,
                );
                current_x += 3.5;
            }
        }
        Some(UnderlineStyle::Dashed) => {
            let mut current_x = x;
            while current_x < x + width {
                rects.push_rect(
                    current_x,
                    underline_y,
                    4.0_f32.min(x + width - current_x),
                    1.0,
                    underline_color,
                );
                current_x += 6.0;
            }
        }
        Some(UnderlineStyle::Single) => {
            rects.push_rect(x, underline_y, width, 1.0, underline_color);
        }
        None => {}
    }

    if style.strikethrough {
        let strike_color = override_color.map_or_else(
            || style.text_color(None, brightness),
            |(color, alpha)| {
                let mut resolved = style.text_color(Some(color), brightness);
                resolved[3] = alpha;
                resolved
            },
        );
        let strike_y = (y + cell_height * 0.52).round();
        rects.push_rect(x, strike_y, width, 1.0, strike_color);
    }
}

fn resolve_underline(attrs: CellAttrs) -> Option<UnderlineStyle> {
    if attrs.contains(CellAttrs::CURLY_UNDERLINE) {
        Some(UnderlineStyle::Curly)
    } else if attrs.contains(CellAttrs::DOUBLE_UNDERLINE) {
        Some(UnderlineStyle::Double)
    } else if attrs.contains(CellAttrs::DOTTED_UNDERLINE) {
        Some(UnderlineStyle::Dotted)
    } else if attrs.contains(CellAttrs::DASHED_UNDERLINE) {
        Some(UnderlineStyle::Dashed)
    } else if attrs.contains(CellAttrs::UNDERLINE) {
        Some(UnderlineStyle::Single)
    } else {
        None
    }
}

pub(super) fn visible_blink_for_cell(cell: &Cell) -> VisibleTextBlink {
    let has_foreground_paint = !cell.attrs.contains(CellAttrs::HIDDEN)
        && (!cell.content.is_empty()
            || cell.attrs.has_underline()
            || cell.attrs.contains(CellAttrs::STRIKE));
    if !has_foreground_paint {
        return VisibleTextBlink::default();
    }
    VisibleTextBlink {
        slow: cell.attrs.contains(CellAttrs::BLINK),
        rapid: cell.attrs.contains(CellAttrs::RAPID_BLINK),
    }
}

fn shade_foreground(color: [f32; 4], dim: bool, brightness: f32) -> [f32; 4] {
    scale_rgb(color, brightness * if dim { DIM_FACTOR } else { 1.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use termeric_core::grid::ColorSpec;

    fn palette() -> TerminalPalette {
        TerminalPalette {
            default_fg: TermColor::rgb(200, 180, 160),
            default_bg: TermColor::rgb(20, 15, 10),
            ansi: std::array::from_fn(|index| {
                TermColor::rgb(index as u8, index as u8 + 1, index as u8 + 2)
            }),
        }
    }

    #[test]
    fn bold_bright_is_explicit_and_precedes_inverse() {
        let mut cell = Cell::with_char('x');
        cell.fg_spec = ColorSpec::Indexed(2);
        cell.bg_spec = ColorSpec::Indexed(4);
        cell.attrs = CellAttrs::BOLD | CellAttrs::INVERSE;

        let enabled = ResolvedCellStyle::resolve(
            &palette(),
            &termeric_core::DynamicColors::default(),
            &cell,
            true,
            TextBlinkPhases::default(),
        );
        assert_eq!(enabled.background, palette().ansi[10]);
        assert_eq!(enabled.foreground, palette().ansi[4]);

        let disabled = ResolvedCellStyle::resolve(
            &palette(),
            &termeric_core::DynamicColors::default(),
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        assert_eq!(disabled.background, palette().ansi[2]);
    }

    #[test]
    fn dim_and_hidden_or_blink_only_change_foreground_paint() {
        let mut cell = Cell::with_char('x');
        cell.attrs = CellAttrs::DIM | CellAttrs::HIDDEN;
        let hidden = ResolvedCellStyle::resolve(
            &palette(),
            &termeric_core::DynamicColors::default(),
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        assert!(!hidden.paint_foreground);
        assert_eq!(hidden.background, palette().default_bg);
        assert!(hidden.text_color(None, 1.0)[0] < 200.0 / 255.0);

        cell.attrs = CellAttrs::BLINK;
        let off = ResolvedCellStyle::resolve(
            &palette(),
            &termeric_core::DynamicColors::default(),
            &cell,
            false,
            TextBlinkPhases {
                slow_visible: false,
                rapid_visible: true,
            },
        );
        assert!(!off.paint_foreground);
    }

    #[test]
    fn every_underline_style_maps_to_one_decoration() {
        for (attr, expected) in [
            (CellAttrs::UNDERLINE, UnderlineStyle::Single),
            (CellAttrs::DOUBLE_UNDERLINE, UnderlineStyle::Double),
            (CellAttrs::CURLY_UNDERLINE, UnderlineStyle::Curly),
            (CellAttrs::DOTTED_UNDERLINE, UnderlineStyle::Dotted),
            (CellAttrs::DASHED_UNDERLINE, UnderlineStyle::Dashed),
        ] {
            assert_eq!(resolve_underline(attr), Some(expected));
        }
    }

    #[test]
    fn inverse_and_underline_color_follow_xterm_order() {
        let mut cell = Cell::with_char('x');
        cell.fg_spec = ColorSpec::Indexed(1);
        cell.bg_spec = ColorSpec::Indexed(2);
        cell.attrs = CellAttrs::INVERSE | CellAttrs::UNDERLINE;
        let resolved = ResolvedCellStyle::resolve(
            &palette(),
            &termeric_core::DynamicColors::default(),
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        assert_eq!(resolved.foreground, palette().ansi[2]);
        assert_eq!(resolved.underline_color, palette().ansi[2]);

        cell.underline_color = Some(TermColor::rgb(12, 34, 56));
        let explicit = ResolvedCellStyle::resolve(
            &palette(),
            &termeric_core::DynamicColors::default(),
            &cell,
            false,
            TextBlinkPhases::default(),
        );
        assert_eq!(explicit.underline_color, TermColor::rgb(12, 34, 56));
    }

    #[test]
    fn only_visible_foreground_blink_schedules_frames() {
        let mut cell = Cell::with_char('x');
        cell.attrs = CellAttrs::BLINK;
        assert_eq!(
            visible_blink_for_cell(&cell),
            VisibleTextBlink {
                slow: true,
                rapid: false,
            }
        );
        cell.attrs = CellAttrs::HIDDEN | CellAttrs::RAPID_BLINK;
        assert_eq!(visible_blink_for_cell(&cell), VisibleTextBlink::default());
        cell.content = termeric_core::CellContent::Empty;
        cell.attrs = CellAttrs::RAPID_BLINK;
        assert_eq!(visible_blink_for_cell(&cell), VisibleTextBlink::default());
    }
}
