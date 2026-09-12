//! Terminal cursor geometry and paint helpers.

use super::*;
use termeric_core::{ExtraCursorColor, ExtraCursorShape, ExtraCursorState};

impl Renderer {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_extra_cursors(
        &mut self,
        grid: &Grid,
        state: &ExtraCursorState,
        dynamic_colors: &termeric_core::DynamicColors,
        offset_x: f32,
        offset_y: f32,
        max_grid_y: f32,
        pane_clip: TextClip,
        main_shape: CursorStyle,
        cursor_opacity: f32,
        brightness: f32,
        text_blink: TextBlinkPhases,
    ) {
        if cursor_opacity <= 0.0 || grid.scroll_offset() != 0 {
            return;
        }
        for cursor in state.cursors() {
            let shape = effective_extra_cursor_shape(cursor.shape, main_shape);
            let Some(cell) = grid.cell(cursor.row, cursor.col) else {
                continue;
            };
            let style = ResolvedCellStyle::resolve(
                &self.terminal_palette,
                dynamic_colors,
                cell,
                self.bold_is_bright,
                text_blink,
            );
            let colors = state.colors();
            let resolved_cursor = self.resolve_extra_cursor_color(
                colors.cursor,
                dynamic_colors,
                dynamic_colors
                    .cursor()
                    .map_or_else(|| self.ui.cursor, |color| color.to_f32_array()),
            );
            let resolved_text =
                self.resolve_extra_cursor_color(colors.text, dynamic_colors, self.ui.cursor_text);
            let (cursor_color, text_color) = extra_cursor_color_pair(
                colors,
                term_color_array(style.foreground),
                term_color_array(style.background),
                resolved_cursor,
                resolved_text,
            );
            self.draw_cursor_at(
                grid,
                dynamic_colors,
                cursor.row,
                cursor.col,
                offset_x,
                offset_y,
                max_grid_y,
                pane_clip,
                shape,
                cursor_opacity,
                brightness,
                text_blink,
                cursor_color,
                text_color,
            );
        }
    }

    fn resolve_extra_cursor_color(
        &self,
        color: ExtraCursorColor,
        dynamic_colors: &termeric_core::DynamicColors,
        fallback: [f32; 4],
    ) -> [f32; 4] {
        match color {
            ExtraCursorColor::Unset | ExtraCursorColor::Special => fallback,
            ExtraCursorColor::Rgb(color) => color.to_f32_array(),
            ExtraCursorColor::Indexed(index) => dynamic_colors
                .palette(index)
                .unwrap_or_else(|| {
                    if index < 16 {
                        self.terminal_palette.ansi[index as usize]
                    } else {
                        termeric_core::ansi::color_256(index)
                    }
                })
                .to_f32_array(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_cursor_at(
        &mut self,
        grid: &Grid,
        dynamic_colors: &termeric_core::DynamicColors,
        row: usize,
        col: usize,
        offset_x: f32,
        offset_y: f32,
        max_grid_y: f32,
        pane_clip: TextClip,
        shape: CursorStyle,
        opacity: f32,
        brightness: f32,
        text_blink: TextBlinkPhases,
        cursor_color: [f32; 4],
        cursor_text: [f32; 4],
    ) {
        let (cursor_top, cursor_left, cursor_rows, cursor_columns) =
            cursor_cell_geometry(grid, row, col);
        let x = offset_x + cursor_left as f32 * self.cell_width;
        let y = offset_y + cursor_top as f32 * self.cell_height;
        let cursor_width = cursor_columns as f32 * self.cell_width;
        let cursor_height = cursor_rows as f32 * self.cell_height;
        if y + cursor_height > max_grid_y {
            return;
        }

        let mut painted_cursor = apply_brightness(cursor_color, brightness);
        painted_cursor[3] = opacity;
        match shape {
            CursorStyle::Block => {
                self.rect_pipeline.push_rounded_rect(
                    x,
                    y,
                    cursor_width,
                    cursor_height,
                    painted_cursor,
                    2.0,
                );
                let Some(cell) = grid.cell_scrolled(cursor_top, cursor_left) else {
                    return;
                };
                let style = ResolvedCellStyle::resolve(
                    &self.terminal_palette,
                    dynamic_colors,
                    &cell,
                    self.bold_is_bright,
                    text_blink,
                );
                if !style.paint_foreground {
                    return;
                }
                let mut color = style.text_color(Some(cursor_text), brightness);
                color[3] = opacity;
                if cell.multicell.is_leader() {
                    self.text_pipeline.push_styled_multicell(
                        &cell.content.to_text(),
                        x,
                        y,
                        color,
                        style.font,
                        pane_clip,
                        self.cell_width,
                        self.cell_height,
                        cell.multicell,
                    );
                } else {
                    match &cell.content {
                        CellContent::Char(character)
                            if !character.is_whitespace()
                                && *character != termeric_core::IMAGE_PLACEHOLDER =>
                        {
                            self.text_pipeline
                                .push_styled_char(*character, x, y, color, style.font, pane_clip);
                        }
                        CellContent::Grapheme(grapheme)
                            if !grapheme.starts_with(termeric_core::IMAGE_PLACEHOLDER)
                                && !grapheme.chars().all(char::is_whitespace) =>
                        {
                            self.text_pipeline
                                .push_styled_grapheme(grapheme, x, y, color, style.font, pane_clip);
                        }
                        _ => {}
                    }
                }
                self.draw_sgr_decorations(
                    style,
                    x,
                    y,
                    cursor_width,
                    brightness,
                    Some((cursor_text, opacity)),
                );
            }
            CursorStyle::Underline => {
                let height = 2.0_f32.max(self.cell_height * 0.1);
                self.rect_pipeline.push_rect(
                    x,
                    y + cursor_height - height,
                    cursor_width,
                    height,
                    painted_cursor,
                );
            }
            CursorStyle::Beam => {
                let width = 2.0_f32.max(self.cell_width * 0.1);
                self.rect_pipeline
                    .push_rect(x, y, width, cursor_height, painted_cursor);
            }
        }
    }
}

fn term_color_array(color: termeric_core::Color) -> [f32; 4] {
    color.to_f32_array()
}

fn effective_extra_cursor_shape(shape: ExtraCursorShape, main: CursorStyle) -> CursorStyle {
    match shape {
        ExtraCursorShape::Block => CursorStyle::Block,
        ExtraCursorShape::Beam => CursorStyle::Beam,
        ExtraCursorShape::Underline => CursorStyle::Underline,
        ExtraCursorShape::FollowMain => main,
    }
}

fn extra_cursor_color_pair(
    colors: termeric_core::ExtraCursorColors,
    cell_foreground: [f32; 4],
    cell_background: [f32; 4],
    resolved_cursor: [f32; 4],
    resolved_text: [f32; 4],
) -> ([f32; 4], [f32; 4]) {
    if colors.cursor == ExtraCursorColor::Special {
        (cell_foreground, cell_background)
    } else if colors.text == ExtraCursorColor::Special {
        (resolved_cursor, cell_background)
    } else {
        (resolved_cursor, resolved_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follow_main_tracks_each_effective_shape() {
        for main in [
            CursorStyle::Block,
            CursorStyle::Beam,
            CursorStyle::Underline,
        ] {
            assert_eq!(
                effective_extra_cursor_shape(ExtraCursorShape::FollowMain, main),
                main
            );
        }
        assert_eq!(
            effective_extra_cursor_shape(ExtraCursorShape::Beam, CursorStyle::Block),
            CursorStyle::Beam
        );
    }

    #[test]
    fn special_cursor_color_has_precedence_over_special_text_color() {
        let foreground = [0.1, 0.2, 0.3, 1.0];
        let background = [0.4, 0.5, 0.6, 1.0];
        let cursor = [0.7, 0.8, 0.9, 1.0];
        let text = [0.9, 0.8, 0.7, 1.0];
        assert_eq!(
            extra_cursor_color_pair(
                termeric_core::ExtraCursorColors {
                    cursor: ExtraCursorColor::Special,
                    text: ExtraCursorColor::Rgb(termeric_core::Color::rgb(1, 2, 3)),
                },
                foreground,
                background,
                cursor,
                text,
            ),
            (foreground, background)
        );
        assert_eq!(
            extra_cursor_color_pair(
                termeric_core::ExtraCursorColors {
                    cursor: ExtraCursorColor::Unset,
                    text: ExtraCursorColor::Special,
                },
                foreground,
                background,
                cursor,
                text,
            ),
            (cursor, background)
        );
    }
}
