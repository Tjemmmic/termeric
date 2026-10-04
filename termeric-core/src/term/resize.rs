//! Resizing the terminal and its pixel geometry.

use super::*;

impl Terminal {
    /// Resize the terminal
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let prompt_navigation_row = self
            .prompt_navigation
            .filter(|navigation| navigation.scroll_offset == self.grid.scroll_offset())
            .map(|navigation| navigation.row);
        let mut shell_rows = self.prompt_tracker.tracked_rows();
        shell_rows.extend(prompt_navigation_row);
        let shell_anchors = capture_shell_anchors(&self.grid, shell_rows);
        let same_columns = self.cols == cols;
        if !same_columns {
            self.navigation_cursor = None;
        }
        let mut primary_points = if !same_columns && !self.alt_screen_active {
            self.selection_reflow_points()
        } else {
            [None; 2]
        };
        let primary_resize = self
            .grid
            .resize_with_points(cols, rows, &mut primary_points);
        let primary_shift = primary_resize.archived_rows;
        let resize_blank = Cell::default();
        if !self.alt_screen_active {
            if same_columns {
                self.remap_selection_after_height_resize(primary_shift, rows);
            } else {
                self.restore_reflow_selection(primary_points);
            }
            self.repair_multicells_with_selection(&resize_blank);
        } else {
            self.grid.repair_multicells(&resize_blank);
        }
        let retained_shell_anchors = shell_anchors
            .into_iter()
            .filter_map(|(old_row, anchor)| {
                anchor
                    .retained_after_resize(primary_resize)
                    .map(|anchor| (old_row, anchor))
            })
            .collect();
        let remapped_shell_rows = restore_shell_anchors(&self.grid, retained_shell_anchors);
        self.prompt_tracker.remap_rows(&remapped_shell_rows);
        self.prompt_navigation = prompt_navigation_row
            .and_then(|row| remapped_shell_rows.get(&row).copied())
            .filter(|row| {
                self.prompt_tracker
                    .all_prompts()
                    .any(|prompt| prompt == *row)
            })
            .map(|row| PromptNavigationState {
                row,
                scroll_offset: self.grid.scroll_offset(),
            });
        let mut alternate_points = if !same_columns && self.alt_screen_active {
            self.selection_reflow_points()
        } else {
            [None; 2]
        };
        let alternate_shift = self
            .alt_grid
            .resize_with_points(cols, rows, &mut alternate_points)
            .archived_rows;
        if self.alt_screen_active {
            if same_columns {
                self.remap_selection_after_height_resize(alternate_shift, rows);
            } else {
                self.restore_reflow_selection(alternate_points);
            }
            self.repair_multicells_with_selection(&resize_blank);
        } else {
            self.alt_grid.repair_multicells(&resize_blank);
        }
        if same_columns {
            self.graphics.adjust_for_height_resize(
                false,
                primary_shift,
                self.grid.scrollback_len(),
            );
            self.graphics.adjust_for_height_resize(
                true,
                alternate_shift,
                self.alt_grid.scrollback_len(),
            );
        } else {
            // Ordinary raster placements retain fixed coordinates across a
            // width change (unlike grid-owned Unicode placeholders). Reflow
            // can still shorten history, so retire now-unreachable roots.
            self.graphics
                .prune_scrollback(false, self.grid.scrollback_len());
            self.graphics
                .prune_scrollback(true, self.alt_grid.scrollback_len());
        }
        if let Some(snapshot) = &mut self.synchronized_output_snapshot {
            if snapshot.grid.cols() != cols || snapshot.grid.rows() != rows {
                snapshot.selection = None;
            }
            let same_snapshot_columns = snapshot.grid.cols() == cols;
            let archived = snapshot.grid.resize_with_row_shift(cols, rows);
            snapshot.graphics.resize(
                same_snapshot_columns,
                archived,
                snapshot.grid.scrollback_len(),
            );
            snapshot.grid.repair_multicells(&resize_blank);
        }
        self.resize_extra_cursors(cols, rows);

        // Preserve customized stops that still fit. Newly exposed columns get
        // the DEC default every eight columns; a shrink followed by a grow
        // therefore restores defaults only in the reintroduced region.
        let old_cols = self.tabs.len();
        self.tabs.resize(cols, false);
        if cols > old_cols {
            let first_default = old_cols.div_ceil(TAB_INTERVAL) * TAB_INTERVAL;
            for i in (first_default.max(TAB_INTERVAL)..cols).step_by(TAB_INTERVAL) {
                self.tabs[i] = true;
            }
        }

        // Update scroll region
        self.scroll_top = 0;
        self.scroll_bottom = rows;
        self.scroll_left = 0;
        self.scroll_right = cols;

        self.cols = cols;
        self.rows = rows;
    }

    /// Update the physical drawable extent represented by the current grid.
    /// Cell values are rounded up and never become zero.
    pub fn set_pixel_geometry(&mut self, pixel_width: u32, pixel_height: u32) {
        self.update_pixel_geometry(self.cols, self.rows, pixel_width, pixel_height);
    }

    /// Apply one host geometry change. Pixel footprints must use the new cell
    /// size before resizing can retire placements outside retained rows.
    /// Pixel-only changes must not reset margins, cursor, or selection state.
    pub fn resize_with_pixel_geometry(
        &mut self,
        cols: usize,
        rows: usize,
        pixel_width: u32,
        pixel_height: u32,
    ) {
        self.update_pixel_geometry(cols, rows, pixel_width, pixel_height);
        if (self.cols, self.rows) != (cols, rows) {
            self.resize(cols, rows);
        }
    }

    fn update_pixel_geometry(
        &mut self,
        cols: usize,
        rows: usize,
        pixel_width: u32,
        pixel_height: u32,
    ) {
        self.grid_pixel_width = pixel_width.max(1);
        self.grid_pixel_height = pixel_height.max(1);
        self.cell_pixel_width = self.grid_pixel_width.div_ceil(cols.max(1) as u32).max(1);
        self.cell_pixel_height = self.grid_pixel_height.div_ceil(rows.max(1) as u32).max(1);
        self.graphics
            .set_cell_pixel_geometry(self.cell_pixel_width, self.cell_pixel_height);
        if let Some(snapshot) = &mut self.synchronized_output_snapshot {
            snapshot
                .graphics
                .set_cell_pixel_geometry(self.cell_pixel_width, self.cell_pixel_height);
        }
    }
}
