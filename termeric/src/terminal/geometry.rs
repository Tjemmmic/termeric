//! Shared physical terminal-grid geometry.

use termeric_core::pty::PtySize;

/// Cell and pixel dimensions describing the exact same drawable grid extent.
///
/// Winit and WGPU dimensions are physical pixels. Cell counts are floored from
/// the available viewport; the represented pixel extent is then the rounded
/// `cells * physical_cell_size`, excluding any unused partial-cell remainder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalGeometry {
    pub cols: usize,
    pub rows: usize,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl TerminalGeometry {
    pub(crate) fn from_window_pixels(
        window_width: u32,
        window_height: u32,
        horizontal_padding: f32,
        horizontal_chrome: f32,
        vertical_chrome: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let available_width =
            (window_width as f32 - horizontal_chrome - horizontal_padding * 2.0).max(0.0);
        let available_height =
            (window_height as f32 - vertical_chrome - horizontal_padding * 2.0).max(0.0);
        Self::from_available_pixels(available_width, available_height, cell_width, cell_height)
    }

    pub(crate) fn from_available_pixels(
        available_width: f32,
        available_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let cell_width = valid_cell_extent(cell_width);
        let cell_height = valid_cell_extent(cell_height);
        let cols = cell_count(available_width, cell_width);
        let rows = cell_count(available_height, cell_height);
        Self::from_cells(cols, rows, cell_width, cell_height)
    }

    pub(crate) fn from_cells(cols: usize, rows: usize, cell_width: f32, cell_height: f32) -> Self {
        let cols = cols.clamp(1, u16::MAX as usize);
        let rows = rows.clamp(1, u16::MAX as usize);
        Self {
            cols,
            rows,
            pixel_width: represented_pixels(cols, valid_cell_extent(cell_width)),
            pixel_height: represented_pixels(rows, valid_cell_extent(cell_height)),
        }
    }

    pub(crate) fn pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows as u16,
            cols: self.cols as u16,
            pixel_width: self.pixel_width,
            pixel_height: self.pixel_height,
        }
    }
}

/// The area a tab's terminals share: its whole-cell grid geometry and the
/// cell size that grid was measured with. Its pixel extent is the cells'
/// own, `cols * cell_width` by `rows * cell_height`, so a terminal filling
/// the area takes `geometry` exactly instead of re-deriving it from pixels,
/// which rounding can shrink by a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalArea {
    pub geometry: TerminalGeometry,
    pub cell_width: f32,
    pub cell_height: f32,
}

impl TerminalArea {
    pub(crate) fn new(geometry: TerminalGeometry, cell_width: f32, cell_height: f32) -> Self {
        Self {
            geometry,
            cell_width,
            cell_height,
        }
    }

    /// The area holding as many whole cells as fit in `width` by `height`.
    #[cfg(test)]
    pub(crate) fn from_available_pixels(
        width: f32,
        height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        Self::new(
            TerminalGeometry::from_available_pixels(width, height, cell_width, cell_height),
            cell_width,
            cell_height,
        )
    }

    pub(crate) fn width(self) -> f32 {
        self.geometry.cols as f32 * self.cell_width
    }

    pub(crate) fn height(self) -> f32 {
        self.geometry.rows as f32 * self.cell_height
    }
}

fn valid_cell_extent(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        1.0
    }
}

fn cell_count(available: f32, cell: f32) -> usize {
    if !available.is_finite() || available <= 0.0 {
        return 1;
    }
    (available / cell).floor().clamp(1.0, f32::from(u16::MAX)) as usize
}

fn represented_pixels(cells: usize, cell: f32) -> u16 {
    (cells as f32 * cell)
        .round()
        .clamp(1.0, f32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_cells_floor_counts_and_round_the_represented_extent() {
        let geometry = TerminalGeometry::from_available_pixels(503.7, 301.2, 8.4, 17.25);
        assert_eq!((geometry.cols, geometry.rows), (59, 17));
        assert_eq!((geometry.pixel_width, geometry.pixel_height), (496, 293));
        assert_eq!(
            geometry.pty_size(),
            PtySize {
                cols: 59,
                rows: 17,
                pixel_width: 496,
                pixel_height: 293,
            }
        );
    }

    #[test]
    fn window_geometry_accounts_for_physical_padding_and_tab_bar() {
        let geometry =
            TerminalGeometry::from_window_pixels(1_000, 700, 11.5, 200.0, 37.0, 9.25, 18.5);
        assert_eq!((geometry.cols, geometry.rows), (84, 34));
        assert_eq!((geometry.pixel_width, geometry.pixel_height), (777, 629));
    }

    #[test]
    fn dimensions_are_nonzero_and_saturate_to_the_pty_abi() {
        let tiny = TerminalGeometry::from_available_pixels(0.0, f32::NAN, 0.0, -1.0);
        assert_eq!(tiny, TerminalGeometry::from_cells(1, 1, 1.0, 1.0));
        let huge = TerminalGeometry::from_cells(usize::MAX, usize::MAX, 2.0, 3.0);
        assert_eq!(huge.cols, u16::MAX as usize);
        assert_eq!(huge.rows, u16::MAX as usize);
        assert_eq!(huge.pixel_width, u16::MAX);
        assert_eq!(huge.pixel_height, u16::MAX);
    }
}
