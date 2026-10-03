//! How panes split, in the one spelling every pane tree shares: the GUI's,
//! a saved workspace's, the control protocol's, a plugin's, and the live
//! domain daemon's.

use serde::{Deserialize, Serialize};

/// Which way a split divides its area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    /// A horizontal divider: panes stacked top to bottom.
    Horizontal,
    /// A vertical divider: panes side by side.
    Vertical,
}

impl SplitDirection {
    /// The other direction, for layouts that alternate.
    pub fn crossed(self) -> Self {
        match self {
            Self::Horizontal => Self::Vertical,
            Self::Vertical => Self::Horizontal,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

/// A layout that rearranges a tab's existing panes without replacing any.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneLayoutPreset {
    /// Side by side.
    Columns,
    /// Stacked.
    Rows,
    /// Alternating splits.
    Grid,
    /// The focused pane on the left, the others stacked on the right.
    Tall,
    /// The focused pane on top, the others side by side below.
    Fat,
}

/// A preset's layout before a pane tree gives its splits identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutShape<T> {
    Pane(T),
    Split {
        direction: SplitDirection,
        /// The first child's share of the split, in thousandths.
        ratio_millis: u16,
        first: Box<LayoutShape<T>>,
        second: Box<LayoutShape<T>>,
    },
}

impl PaneLayoutPreset {
    /// The layout this preset gives `panes`, kept in order except that Tall
    /// and Fat put `focused` first. `None` for fewer than two panes, or for
    /// Tall and Fat when `focused` is not one of them.
    pub fn shape<T: Copy + PartialEq>(self, panes: &[T], focused: T) -> Option<LayoutShape<T>> {
        if panes.len() < 2 {
            return None;
        }
        match self {
            Self::Columns => Some(even_shape(panes, SplitDirection::Vertical, false)),
            Self::Rows => Some(even_shape(panes, SplitDirection::Horizontal, false)),
            Self::Grid => Some(even_shape(panes, SplitDirection::Vertical, true)),
            Self::Tall => master_detail_shape(panes, focused, SplitDirection::Vertical),
            Self::Fat => master_detail_shape(panes, focused, SplitDirection::Horizontal),
        }
    }
}

/// `panes` halved recursively, each half getting its share of the area;
/// `alternate` crosses the direction at every level.
fn even_shape<T: Copy>(panes: &[T], direction: SplitDirection, alternate: bool) -> LayoutShape<T> {
    if let [pane] = panes {
        return LayoutShape::Pane(*pane);
    }
    let midpoint = panes.len().div_ceil(2);
    let next_direction = if alternate {
        direction.crossed()
    } else {
        direction
    };
    let ratio_millis = u16::try_from((midpoint * 1_000 + panes.len() / 2) / panes.len())
        .expect("a half share is at most 1000 thousandths");
    LayoutShape::Split {
        direction,
        ratio_millis,
        first: Box::new(even_shape(&panes[..midpoint], next_direction, alternate)),
        second: Box::new(even_shape(&panes[midpoint..], next_direction, alternate)),
    }
}

/// `master` in one half, the other panes tiled evenly across the other.
fn master_detail_shape<T: Copy + PartialEq>(
    panes: &[T],
    master: T,
    direction: SplitDirection,
) -> Option<LayoutShape<T>> {
    if !panes.contains(&master) {
        return None;
    }
    let details = panes
        .iter()
        .copied()
        .filter(|pane| *pane != master)
        .collect::<Vec<_>>();
    Some(LayoutShape::Split {
        direction,
        ratio_millis: 500,
        first: Box::new(LayoutShape::Pane(master)),
        second: Box::new(even_shape(&details, direction.crossed(), false)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split<T>(
        direction: SplitDirection,
        ratio_millis: u16,
        first: LayoutShape<T>,
        second: LayoutShape<T>,
    ) -> LayoutShape<T> {
        LayoutShape::Split {
            direction,
            ratio_millis,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    #[test]
    fn presets_halve_in_order_and_alternate_only_for_grid() {
        use LayoutShape::Pane;
        use SplitDirection::{Horizontal, Vertical};
        assert_eq!(
            PaneLayoutPreset::Columns.shape(&[1, 2, 3], 1),
            Some(split(
                Vertical,
                667,
                split(Vertical, 500, Pane(1), Pane(2)),
                Pane(3)
            ))
        );
        assert_eq!(
            PaneLayoutPreset::Rows.shape(&[1, 2], 2),
            Some(split(Horizontal, 500, Pane(1), Pane(2)))
        );
        assert_eq!(
            PaneLayoutPreset::Grid.shape(&[1, 2, 3, 4], 1),
            Some(split(
                Vertical,
                500,
                split(Horizontal, 500, Pane(1), Pane(2)),
                split(Horizontal, 500, Pane(3), Pane(4))
            ))
        );
        assert_eq!(PaneLayoutPreset::Grid.shape(&[1], 1), None);
    }

    #[test]
    fn master_detail_puts_the_focused_pane_beside_the_rest() {
        use LayoutShape::Pane;
        use SplitDirection::{Horizontal, Vertical};
        assert_eq!(
            PaneLayoutPreset::Tall.shape(&[3, 4, 5, 6], 5),
            Some(split(
                Vertical,
                500,
                Pane(5),
                split(
                    Horizontal,
                    667,
                    split(Horizontal, 500, Pane(3), Pane(4)),
                    Pane(6)
                )
            ))
        );
        assert_eq!(
            PaneLayoutPreset::Fat.shape(&[3, 4], 4),
            Some(split(Horizontal, 500, Pane(4), Pane(3)))
        );
        assert_eq!(PaneLayoutPreset::Tall.shape(&[3, 4], 99), None);
    }

    #[test]
    fn wire_spellings_are_fixed() {
        for (direction, name) in [
            (SplitDirection::Horizontal, "horizontal"),
            (SplitDirection::Vertical, "vertical"),
        ] {
            assert_eq!(direction.name(), name);
            assert_eq!(
                serde_json::to_string(&direction).unwrap(),
                format!("\"{name}\"")
            );
            assert_eq!(
                serde_json::from_str::<SplitDirection>(&format!("\"{name}\"")).unwrap(),
                direction
            );
            assert_eq!(direction.crossed().crossed(), direction);
            assert_ne!(direction.crossed(), direction);
        }
        for (preset, name) in [
            (PaneLayoutPreset::Columns, "columns"),
            (PaneLayoutPreset::Rows, "rows"),
            (PaneLayoutPreset::Grid, "grid"),
            (PaneLayoutPreset::Tall, "tall"),
            (PaneLayoutPreset::Fat, "fat"),
        ] {
            assert_eq!(
                serde_json::to_string(&preset).unwrap(),
                format!("\"{name}\"")
            );
            assert_eq!(
                serde_json::from_str::<PaneLayoutPreset>(&format!("\"{name}\"")).unwrap(),
                preset
            );
        }
    }
}
