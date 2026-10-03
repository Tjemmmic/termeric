//! One scoped pair of modal coordinates, mapped by the actual resize pass.

use super::Grid;
use crate::{Point, Selection, SelectionType};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TRACKING_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque owner token for one grid's pending resize transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResizeTrackingToken(u64);

pub(super) struct ResizeTracking {
    token: ResizeTrackingToken,
    pub revision: u64,
    /// Cursor first, optional selection anchor second.
    pub points: [Option<(i32, usize)>; 2],
    pub selection_type: SelectionType,
}

impl ResizeTracking {
    pub(crate) fn damage(&mut self, rows: Range<usize>, cols: Range<usize>, width: usize) {
        let Some((row, col)) = self.points[0] else {
            return;
        };
        let cursor = Point::new(row, col);
        if let Some((row, col)) = self.points[1] {
            let mut selection = Selection::new(Point::new(row, col), self.selection_type);
            selection.update(cursor);
            if selection.intersects_region(rows.clone(), cols.clone(), width) {
                self.points[1] = None;
            }
        }
        if Selection::new(cursor, SelectionType::Simple).intersects_region(rows, cols, width) {
            self.points = [None; 2];
        }
    }
}

impl Grid {
    /// Begin a synchronous resize transaction for a modal cursor and anchor.
    /// A newer request replaces the old one. Unrelated content mutation or a
    /// different grid invalidates the token's result; navigation is harmless.
    /// Only two coordinates are stored, never text or a retained-history map.
    pub fn begin_resize_tracking(
        &mut self,
        cursor: Point,
        anchor: Option<Point>,
        selection_type: SelectionType,
    ) -> ResizeTrackingToken {
        let token = ResizeTrackingToken(NEXT_TRACKING_ID.fetch_add(1, Ordering::Relaxed));
        self.resize_tracking = Some(ResizeTracking {
            token,
            revision: self.content_revision,
            points: [Some(cursor), anchor].map(|point| point.map(|point| (point.row, point.col))),
            selection_type,
        });
        token
    }

    /// Consume this request's result. Missing endpoints were discarded, not
    /// clamped onto retained text. Wrong tokens cannot consume another request.
    pub fn finish_resize_tracking(
        &mut self,
        token: ResizeTrackingToken,
    ) -> Option<[Option<Point>; 2]> {
        if self.resize_tracking.as_ref()?.token != token {
            return None;
        }
        let tracking = self.take_valid_resize_tracking()?;
        Some(
            tracking
                .points
                .map(|point| point.map(|(row, col)| Point::new(row, col))),
        )
    }

    pub(super) fn take_valid_resize_tracking(&mut self) -> Option<ResizeTracking> {
        self.resize_tracking
            .take()
            .filter(|tracking| tracking.revision == self.content_revision)
    }

    pub(super) fn restore_resize_tracking(&mut self, tracking: Option<ResizeTracking>) {
        self.resize_tracking = tracking.map(|mut tracking| {
            tracking.revision = self.content_revision;
            tracking
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_is_single_use_grid_bound_and_rejects_intervening_mutations() {
        let mut grid = Grid::new(4, 2);
        grid.cell_mut(0, 0).unwrap().set_char('a');
        let old = grid.begin_resize_tracking(Point::new(0, 0), None, SelectionType::Simple);
        let new = grid.begin_resize_tracking(Point::new(0, 0), None, SelectionType::Simple);
        assert!(grid.finish_resize_tracking(old).is_none());
        let mut other = Grid::new(4, 2);
        assert!(other.finish_resize_tracking(new).is_none());
        grid.resize(2, 3);
        grid.resize(1, 4);
        assert_eq!(
            grid.finish_resize_tracking(new),
            Some([Some(Point::new(0, 0)), None])
        );
        assert!(grid.finish_resize_tracking(new).is_none());

        for resize_first in [false, true] {
            let token = grid.begin_resize_tracking(Point::new(0, 0), None, SelectionType::Simple);
            if resize_first {
                grid.resize(3, 4);
            }
            grid.cell_mut(0, 0).unwrap().set_char('b');
            if !resize_first {
                grid.resize(2, 4);
            }
            assert!(grid.finish_resize_tracking(token).is_none());
        }
        let token = grid.begin_resize_tracking(Point::new(0, 0), None, SelectionType::Simple);
        grid.set_cursor(1, 0);
        grid.set_scroll_offset(0);
        assert_eq!(
            grid.finish_resize_tracking(token),
            Some([Some(Point::new(0, 0)), None])
        );
    }
}
