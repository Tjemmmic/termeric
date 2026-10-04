//! A terminal grid's rows, cursor, and scrollbar, cached row by row.

use super::*;

fn hash_text_clip<H: Hasher>(clip: TextClip, hasher: &mut H) {
    clip.left.to_bits().hash(hasher);
    clip.top.to_bits().hash(hasher);
    clip.right.to_bits().hash(hasher);
    clip.bottom.to_bits().hash(hasher);
}

impl Renderer {
    /// Render a single terminal grid within a pane viewport.
    ///
    /// This method handles cell rendering, cursor, scrollbar, selection, search
    /// highlights, underlines, and URL detection for one grid. It is called once
    /// in single-grid mode or once per pane in multi-pane mode.
    pub(super) fn build_pane_grid(
        &mut self,
        grid: &Grid,
        selection: Option<&Selection>,
        dynamic_colors: &termeric_core::DynamicColors,
        extra_cursors: &termeric_core::ExtraCursorState,
        offset_x: f32,
        offset_y: f32,
        pane_cols: usize,
        pane_rows: usize,
        is_focused: bool,
        cursor_style: CursorStyle,
        cursor_opacity: f32,
        cursor_pos: (f32, f32),
        text_blink: TextBlinkPhases,
        copy_flash: f32,
        brightness: f32,
        search: &Search,
        url_cache: &UrlCache,
        window_height: f32,
        pane_width: f32,
    ) {
        let cell_width = self.cell_width();
        let cell_height = self.cell_height();

        // Selection highlight color
        let selection_base = self.ui.selection;
        let selection_flash = self.ui.selection_flash;
        let selection_color =
            scale_rgb(mix(selection_base, selection_flash, copy_flash), brightness);

        let scroll_offset = grid.scroll_offset();
        let scroll_offset_i32 = i32::try_from(scroll_offset).unwrap_or(i32::MAX);
        let default_bg = self.terminal_palette.default_bg(dynamic_colors);
        if dynamic_colors.background().is_some() {
            self.rect_pipeline.push_rect(
                offset_x,
                offset_y,
                pane_width,
                pane_rows as f32 * cell_height,
                default_bg.to_f32_array(),
            );
        }

        let pane_clip = TextClip {
            left: offset_x,
            top: offset_y,
            right: offset_x + pane_width,
            bottom: (offset_y + pane_rows as f32 * cell_height).min(window_height - self.padding),
        };
        let max_grid_y = pane_clip.bottom;

        // Determine which rows to render based on filter mode
        let matching_rows = search.filtered_rows(pane_rows);
        let filter_mode = is_focused && !matching_rows.is_empty();

        let rows_to_render: Vec<(usize, i32)> = if filter_mode {
            matching_rows
                .iter()
                .enumerate()
                .map(|(visual_row, &grid_row)| (visual_row, grid_row))
                .collect()
        } else {
            (0..pane_rows.min(grid.rows()))
                .map(|row| (row, row as i32 - scroll_offset_i32))
                .collect()
        };

        // Text runs break around the cell the cursor rests on, never its
        // gliding position: that changes every frame and can sit between cells.
        let cursor_cell =
            (is_focused && cursor_opacity > 0.0 && scroll_offset == 0 && !filter_mode)
                .then(|| grid.cursor())
                .map(|cursor| (cursor.row, cursor.col));
        for (visual_row, grid_row) in rows_to_render {
            let row_data = if filter_mode {
                grid.row_at_offset(grid_row)
            } else {
                None
            };

            let key = RowCacheKey {
                grid_id: grid as *const Grid as usize,
                visual_row,
            };
            let cursor_break_col =
                cursor_cell.and_then(|(row, col)| (row == visual_row).then_some(col));
            let mut hasher = DefaultHasher::new();
            dynamic_colors.generation().hash(&mut hasher);
            grid_row.hash(&mut hasher);
            pane_cols.hash(&mut hasher);
            is_focused.hash(&mut hasher);
            offset_x.to_bits().hash(&mut hasher);
            offset_y.to_bits().hash(&mut hasher);
            hash_text_clip(pane_clip, &mut hasher);
            cell_width.to_bits().hash(&mut hasher);
            cell_height.to_bits().hash(&mut hasher);
            brightness.to_bits().hash(&mut hasher);
            cursor_break_col.hash(&mut hasher);
            for component in selection_color {
                component.to_bits().hash(&mut hasher);
            }
            let mut row_blink = VisibleTextBlink::default();
            for col in 0..pane_cols.min(grid.cols()) {
                let cell = if filter_mode {
                    row_data.as_ref().and_then(|row| row.get(col))
                } else {
                    grid.cell_scrolled(visual_row, col)
                };
                match cell {
                    Some(cell) => {
                        cell.hash(&mut hasher);
                        let cell_blink = style::visible_blink_for_cell(&cell);
                        row_blink.slow |= cell_blink.slow;
                        row_blink.rapid |= cell_blink.rapid;
                    }
                    None => 0_u8.hash(&mut hasher),
                }
                let selected = is_focused
                    && selection.is_some_and(|selection| selection.contains(grid_row, col));
                selected.hash(&mut hasher);
                (is_focused && search.is_match(grid_row, col)).hash(&mut hasher);
                (is_focused && search.is_current_match(grid_row, col)).hash(&mut hasher);
                url_cache.is_url(visual_row, col).hash(&mut hasher);
            }
            self.visible_text_blink.slow |= row_blink.slow;
            self.visible_text_blink.rapid |= row_blink.rapid;
            if row_blink.slow {
                text_blink.slow_visible.hash(&mut hasher);
            }
            if row_blink.rapid {
                text_blink.rapid_visible.hash(&mut hasher);
            }
            self.glyph_cache.atlas_generation().hash(&mut hasher);
            let signature = hasher.finish();
            if self.terminal_row_cache.reuse(key, signature) {
                let pages = self.terminal_row_cache.referenced_atlas_pages(key);
                self.glyph_cache.pin_pages(&pages);
                continue;
            }

            let rect_checkpoint = self.rect_pipeline.checkpoint();
            let text_checkpoint = self.text_pipeline.queued_checkpoint();
            let mut text_run = TextRunAccumulator::default();

            for col in 0..pane_cols.min(grid.cols()) {
                if cursor_break_col == Some(col) {
                    text_run.flush(&mut self.text_pipeline);
                }
                let cell = if filter_mode {
                    row_data.as_ref().and_then(|r| r.get(col))
                } else {
                    grid.cell_scrolled(visual_row, col)
                };
                let Some(cell) = cell else {
                    text_run.flush(&mut self.text_pipeline);
                    continue;
                };

                let x = offset_x + col as f32 * cell_width;
                let y = offset_y + visual_row as f32 * cell_height;

                if y + cell_height > max_grid_y {
                    text_run.flush(&mut self.text_pipeline);
                    break;
                }

                let is_selected = is_focused
                    && selection
                        .map(|s| s.contains(grid_row, col))
                        .unwrap_or(false);

                let is_search_match = is_focused && search.is_match(grid_row, col);
                let is_current_match = is_focused && search.is_current_match(grid_row, col);

                let style = ResolvedCellStyle::resolve(
                    &self.terminal_palette,
                    dynamic_colors,
                    &cell,
                    self.bold_is_bright,
                    text_blink,
                );

                // Draw background
                if is_selected {
                    self.rect_pipeline
                        .push_rect(x, y, cell_width, cell_height, selection_color);
                } else if is_current_match {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        scale_rgb(self.ui.search_match_current, brightness),
                    );
                } else if is_search_match {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        scale_rgb(self.ui.search_match, brightness),
                    );
                } else if style.background != default_bg {
                    self.rect_pipeline.push_rect(
                        x,
                        y,
                        cell_width,
                        cell_height,
                        scale_rgb(style.background.to_f32_array(), brightness),
                    );
                }

                // Draw the complete grapheme leader. Wide spacers are empty
                // attributed cells and intentionally queue no text.
                if style.paint_foreground && !cell.content.is_empty() {
                    let text_color = style.text_color(
                        is_selected.then_some(self.ui.selection_text).flatten(),
                        brightness,
                    );
                    if cell.multicell.is_leader() {
                        text_run.flush(&mut self.text_pipeline);
                        self.text_pipeline.push_styled_multicell(
                            &cell.content.to_text(),
                            x,
                            y,
                            text_color,
                            style.font,
                            pane_clip,
                            cell_width,
                            cell_height,
                            cell.multicell,
                        );
                    } else if cell.multicell.is_present() {
                        text_run.flush(&mut self.text_pipeline);
                    } else {
                        match &cell.content {
                            CellContent::Char(character)
                                if draws_char(*character)
                                    && !cell.flags.contains(CellFlags::WIDE_CHAR) =>
                            {
                                text_run.push(
                                    &mut self.text_pipeline,
                                    *character,
                                    x,
                                    y,
                                    text_color,
                                    style.font,
                                    pane_clip,
                                    cell_width,
                                );
                            }
                            CellContent::Char(character) if draws_char(*character) => {
                                text_run.flush(&mut self.text_pipeline);
                                self.text_pipeline.push_styled_char(
                                    *character, x, y, text_color, style.font, pane_clip,
                                );
                            }
                            CellContent::Grapheme(grapheme) if draws_grapheme(grapheme) => {
                                text_run.flush(&mut self.text_pipeline);
                                self.text_pipeline.push_styled_grapheme(
                                    grapheme, x, y, text_color, style.font, pane_clip,
                                );
                            }
                            _ => text_run.flush(&mut self.text_pipeline),
                        }
                    }
                } else {
                    text_run.flush(&mut self.text_pipeline);
                }
                if cursor_break_col == Some(col) {
                    text_run.flush(&mut self.text_pipeline);
                }

                if style.paint_foreground {
                    self.draw_sgr_decorations(style, x, y, cell_width, brightness, None);
                }

                // Draw hyperlink underline
                let is_hyperlink = cell.flags.contains(CellFlags::HYPERLINK);
                let is_detected_url = url_cache.is_url(visual_row, col);
                if style.paint_foreground
                    && (is_hyperlink || is_detected_url)
                    && style.underline.is_none()
                {
                    style::push_dotted_line(
                        &mut self.rect_pipeline,
                        x,
                        y + cell_height - 2.0,
                        cell_width,
                        scale_rgb(self.ui.url, brightness),
                    );
                }
            }
            text_run.flush(&mut self.text_pipeline);

            let rects = self.rect_pipeline.take_since(rect_checkpoint);
            let queued = self.text_pipeline.take_queued_since(text_checkpoint);
            let text = TextPipeline::build_batch(&queued, &self.queue, &mut self.glyph_cache);
            self.terminal_row_cache
                .replace(key, signature, rects, text, &self.device, &self.queue);
        }

        // Extra cursors are application-owned and remain visible even when
        // DECTCEM hides the main cursor or this pane is not focused.
        if !filter_mode {
            self.build_extra_cursors(
                grid,
                extra_cursors,
                dynamic_colors,
                offset_x,
                offset_y,
                max_grid_y,
                pane_clip,
                cursor_style,
                cursor_opacity,
                brightness,
                text_blink,
            );
        }

        // Draw cursor (only for focused pane, not scrolled back, not in filter mode)
        if is_focused && cursor_opacity > 0.0 && scroll_offset == 0 && !filter_mode {
            let cursor = grid.cursor();
            if cursor.visible {
                let cursor_color = dynamic_colors
                    .cursor()
                    .map_or(self.ui.cursor, |color| color.to_f32_array());
                self.draw_cursor_at(
                    grid,
                    dynamic_colors,
                    cursor.row,
                    cursor.col,
                    cursor_pos,
                    offset_x,
                    offset_y,
                    max_grid_y,
                    pane_clip,
                    cursor_style,
                    cursor_opacity,
                    brightness,
                    text_blink,
                    cursor_color,
                    self.ui.cursor_text,
                );
            }
        }

        // Draw scrollbar (only when there is scrollback content)
        let scrollback_len = grid.scrollback_len();
        if scrollback_len > 0 {
            let visible_rows = pane_rows.min(grid.rows());
            let total_rows = scrollback_len + visible_rows;

            let scrollbar_width = 6.0_f32;
            let scrollbar_margin = 2.0_f32;
            let scrollbar_x = offset_x + pane_width - scrollbar_width - scrollbar_margin;
            let scrollbar_top = offset_y;
            let scrollbar_bottom = offset_y + pane_rows as f32 * cell_height;
            let scrollbar_height = scrollbar_bottom - scrollbar_top;

            if scrollbar_height > 20.0 {
                let track_color = [1.0_f32, 1.0, 1.0, 0.05];
                self.rect_pipeline.push_rounded_rect(
                    scrollbar_x,
                    scrollbar_top,
                    scrollbar_width,
                    scrollbar_height,
                    track_color,
                    3.0,
                );

                // Minimum thumb size of 32px keeps it targetable on HiDPI
                // displays even when scrollback vastly exceeds the viewport.
                let thumb_ratio = (visible_rows as f32) / (total_rows as f32);
                let thumb_height = (scrollbar_height * thumb_ratio)
                    .max(32.0)
                    .min(scrollbar_height);
                let max_offset = scrollback_len;
                let scroll_fraction = if max_offset > 0 {
                    scroll_offset as f32 / max_offset as f32
                } else {
                    0.0
                };
                let thumb_travel = scrollbar_height - thumb_height;
                let thumb_y = scrollbar_top + thumb_travel * (1.0 - scroll_fraction);

                let thumb_color = [1.0_f32, 1.0, 1.0, 0.25];
                self.rect_pipeline.push_rounded_rect(
                    scrollbar_x,
                    thumb_y,
                    scrollbar_width,
                    thumb_height,
                    thumb_color,
                    3.0,
                );
            }
        }
    }
}

/// Whether a cell holding this character draws it: blanks and Kitty image
/// placeholders have no glyph.
pub(super) fn draws_char(character: char) -> bool {
    !character.is_whitespace() && character != termeric_core::IMAGE_PLACEHOLDER
}

/// Whether a cell holding this grapheme draws it; see [`draws_char`].
pub(super) fn draws_grapheme(grapheme: &str) -> bool {
    !grapheme.starts_with(termeric_core::IMAGE_PLACEHOLDER)
        && !grapheme.chars().all(char::is_whitespace)
}

pub(super) fn cursor_cell_geometry(
    grid: &Grid,
    row: usize,
    col: usize,
) -> (usize, usize, usize, usize) {
    grid.multicell_region_at(row, col)
        .map_or((row, col, 1, 1), |region| {
            (
                region.top,
                region.left,
                region.descriptor.rows(),
                region.descriptor.columns(),
            )
        })
}

/// Where to draw the cursor, in cells: the origin of the cell it rests on,
/// moved by the distance its glide still has to travel. `cursor` is the grid
/// cursor and `glide` the animated position, both `(col, row)`. At rest the
/// result is exactly `rest`, so a cursor inside a scaled cell lands on it.
pub(super) fn gliding_cursor_origin(
    rest: (usize, usize),
    cursor: (usize, usize),
    glide: (f32, f32),
) -> (f32, f32) {
    (
        rest.0 as f32 + glide.0 - cursor.0 as f32,
        rest.1 as f32 + glide.1 - cursor.1 as f32,
    )
}

#[cfg(test)]
#[test]
fn the_cursor_glides_toward_the_cell_it_rests_on() {
    // At rest on a plain cell and on a scaled cell's interior.
    assert_eq!(
        gliding_cursor_origin((5, 2), (5, 2), (5.0, 2.0)),
        (5.0, 2.0)
    );
    assert_eq!(
        gliding_cursor_origin((1, 0), (4, 1), (4.0, 1.0)),
        (1.0, 0.0)
    );
    // Mid-glide, the remaining distance carries over to the scaled origin.
    assert_eq!(
        gliding_cursor_origin((1, 0), (4, 1), (2.5, 3.0)),
        (-0.5, 2.0)
    );
}

#[cfg(test)]
#[test]
fn multicell_cursor_geometry_expands_from_every_covered_cell() {
    let mut terminal = termeric_core::Terminal::new(10, 4);
    terminal.process(b".\x1b]66;s=2:w=2;X\x07");
    assert_eq!(cursor_cell_geometry(terminal.grid(), 0, 1), (0, 1, 2, 4));
    assert_eq!(cursor_cell_geometry(terminal.grid(), 1, 4), (0, 1, 2, 4));
    assert_eq!(cursor_cell_geometry(terminal.grid(), 3, 9), (3, 9, 1, 1));
}

#[cfg(test)]
mod row_cache_signature_tests {
    use super::*;

    fn clip_signature(clip: TextClip) -> u64 {
        let mut hasher = DefaultHasher::new();
        hash_text_clip(clip, &mut hasher);
        hasher.finish()
    }

    #[test]
    fn every_pane_clip_boundary_participates_in_the_row_signature() {
        let base = TextClip {
            left: 1.0,
            top: 2.0,
            right: 101.0,
            bottom: 42.0,
        };
        let base_signature = clip_signature(base);
        for changed in [
            TextClip { left: 1.5, ..base },
            TextClip { top: 2.5, ..base },
            TextClip {
                right: 101.5,
                ..base
            },
            TextClip {
                bottom: 42.5,
                ..base
            },
        ] {
            assert_ne!(clip_signature(changed), base_signature);
        }
    }
}
