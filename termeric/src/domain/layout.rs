//! Tree operations on a live domain tab's pane layout. They live apart from
//! `model.rs`, which the fuzz target includes, so the daemon's edits don't
//! become unused code there.

use super::model::LivePaneLayout;
use anyhow::Context;
use termeric_core::split::{LayoutShape, SplitDirection};

/// The split ID `next_split` holds, advancing it. Split IDs are never
/// reused within a domain, so running out is an error rather than a wrap.
pub(super) fn take_split_id(next_split: &mut u32) -> anyhow::Result<u32> {
    let split_id = *next_split;
    *next_split = split_id
        .checked_add(1)
        .context("domain split identity space exhausted")?;
    Ok(split_id)
}

impl LivePaneLayout {
    /// The layout for a preset's shape, numbering its splits from
    /// `next_split`.
    pub(super) fn from_shape(
        shape: LayoutShape<u32>,
        next_split: &mut u32,
    ) -> anyhow::Result<Self> {
        Ok(match shape {
            LayoutShape::Pane(pane) => Self::Pane { pane },
            LayoutShape::Split {
                direction,
                ratio_millis,
                first,
                second,
            } => Self::Split {
                split_id: take_split_id(next_split)?,
                direction,
                ratio_millis,
                first: Box::new(Self::from_shape(*first, next_split)?),
                second: Box::new(Self::from_shape(*second, next_split)?),
            },
        })
    }

    /// Splits the leaf `target` evenly into itself and `new_pane`, in that
    /// order; false when no leaf is `target`.
    pub(super) fn split_leaf(
        &mut self,
        target: u32,
        new_pane: u32,
        split_id: u32,
        direction: SplitDirection,
    ) -> bool {
        match self {
            Self::Pane { pane } if *pane == target => {
                *self = Self::Split {
                    split_id,
                    direction,
                    ratio_millis: 500,
                    first: Box::new(Self::Pane { pane: target }),
                    second: Box::new(Self::Pane { pane: new_pane }),
                };
                true
            }
            Self::Pane { .. } => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, new_pane, split_id, direction)
                    || second.split_leaf(target, new_pane, split_id, direction)
            }
        }
    }

    /// Sets split `target`'s ratio: whether it changed, or `None` when no
    /// split is `target`.
    pub(super) fn resize_split(&mut self, target: u32, ratio_millis: u16) -> Option<bool> {
        match self {
            Self::Pane { .. } => None,
            Self::Split {
                split_id,
                ratio_millis: ratio,
                first,
                second,
                ..
            } => {
                if *split_id == target {
                    let changed = *ratio != ratio_millis;
                    *ratio = ratio_millis;
                    Some(changed)
                } else {
                    first
                        .resize_split(target, ratio_millis)
                        .or_else(|| second.resize_split(target, ratio_millis))
                }
            }
        }
    }

    /// Every leaf's pane, first to last.
    pub(super) fn panes(&self) -> Vec<u32> {
        let mut panes = Vec::new();
        self.collect_panes(&mut panes);
        panes
    }

    fn collect_panes(&self, panes: &mut Vec<u32>) {
        match self {
            Self::Pane { pane } => panes.push(*pane),
            Self::Split { first, second, .. } => {
                first.collect_panes(panes);
                second.collect_panes(panes);
            }
        }
    }

    /// Exchanges the leaves holding `first_id` and `second_id`.
    pub(super) fn swap_panes(&mut self, first_id: u32, second_id: u32) {
        match self {
            Self::Pane { pane } if *pane == first_id => *pane = second_id,
            Self::Pane { pane } if *pane == second_id => *pane = first_id,
            Self::Pane { .. } => {}
            Self::Split { first, second, .. } => {
                first.swap_panes(first_id, second_id);
                second.swap_panes(first_id, second_id);
            }
        }
    }

    /// Sets every split to an even ratio; whether any changed.
    pub(super) fn balance(&mut self) -> bool {
        match self {
            Self::Pane { .. } => false,
            Self::Split {
                ratio_millis,
                first,
                second,
                ..
            } => {
                let changed = *ratio_millis != 500;
                *ratio_millis = 500;
                first.balance() | second.balance() | changed
            }
        }
    }

    /// The layout without leaf `target`, whose sibling takes its parent's
    /// place; `None` when nothing is left.
    pub(super) fn without_pane(self, target: u32) -> Option<Self> {
        match self {
            Self::Pane { pane } if pane == target => None,
            pane @ Self::Pane { .. } => Some(pane),
            Self::Split {
                split_id,
                direction,
                ratio_millis,
                first,
                second,
            } => match (first.without_pane(target), second.without_pane(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    split_id,
                    direction,
                    ratio_millis,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            },
        }
    }

    /// The first leaf's pane.
    pub(super) fn first_pane(&self) -> u32 {
        match self {
            Self::Pane { pane } => *pane,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    /// The pane that takes focus when leaf `target` leaves: the first leaf
    /// of its sibling. `None` when `target` is not a leaf below a split.
    pub(super) fn sibling_of(&self, target: u32) -> Option<u32> {
        match self {
            Self::Pane { .. } => None,
            Self::Split { first, second, .. } => {
                if matches!(first.as_ref(), Self::Pane { pane } if *pane == target) {
                    return Some(second.first_pane());
                }
                if matches!(second.as_ref(), Self::Pane { pane } if *pane == target) {
                    return Some(first.first_pane());
                }
                first
                    .sibling_of(target)
                    .or_else(|| second.sibling_of(target))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termeric_core::split::PaneLayoutPreset;

    #[test]
    fn split_layout_preserves_existing_branches_and_exact_direction() {
        let mut layout = LivePaneLayout::Pane { pane: 0 };
        assert!(layout.split_leaf(0, 1, 10, SplitDirection::Vertical));
        assert!(layout.split_leaf(1, 2, 11, SplitDirection::Horizontal));
        assert!(!layout.split_leaf(99, 3, 12, SplitDirection::Vertical));
        assert_eq!(layout.resize_split(11, 350), Some(true));
        assert_eq!(layout.resize_split(11, 350), Some(false));
        assert_eq!(layout.resize_split(999, 500), None);
        assert!(matches!(
            &layout,
            LivePaneLayout::Split {
                split_id: 10,
                ratio_millis: 500,
                second,
                ..
            } if matches!(
                second.as_ref(),
                LivePaneLayout::Split {
                    split_id: 11,
                    ratio_millis: 350,
                    ..
                }
            )
        ));
        let encoded = serde_json::to_string(&layout).unwrap();
        assert!(encoded.contains("vertical"));
        assert!(encoded.contains("horizontal"));
    }

    #[test]
    fn closing_a_layout_leaf_collapses_only_its_parent_and_preserves_order() {
        let mut layout = LivePaneLayout::Pane { pane: 0 };
        assert!(layout.split_leaf(0, 1, 10, SplitDirection::Vertical));
        assert!(layout.split_leaf(1, 2, 11, SplitDirection::Horizontal));

        let mut layout = layout.without_pane(1).unwrap();
        assert_eq!(
            layout,
            LivePaneLayout::Split {
                split_id: 10,
                direction: SplitDirection::Vertical,
                ratio_millis: 500,
                first: Box::new(LivePaneLayout::Pane { pane: 0 }),
                second: Box::new(LivePaneLayout::Pane { pane: 2 }),
            }
        );
        assert_eq!(
            layout.resize_split(11, 350),
            None,
            "the collapsed child split ID must become stale"
        );
        assert_eq!(layout.resize_split(10, 650), Some(true));
        assert_eq!(layout.first_pane(), 0);
        assert_eq!(
            layout.without_pane(0),
            Some(LivePaneLayout::Pane { pane: 2 })
        );
        assert_eq!(LivePaneLayout::Pane { pane: 2 }.without_pane(2), None);
    }

    #[test]
    fn pane_layout_operations_preserve_leaf_identity_and_allocate_stable_splits() {
        let mut next_split = 20;
        let shape = PaneLayoutPreset::Grid.shape(&[3, 4, 5], 3).unwrap();
        let mut layout = LivePaneLayout::from_shape(shape, &mut next_split).unwrap();
        assert_eq!(next_split, 22);
        assert!(matches!(
            &layout,
            LivePaneLayout::Split {
                split_id: 20,
                direction: SplitDirection::Vertical,
                ratio_millis: 667,
                first,
                second,
            } if matches!(
                first.as_ref(),
                LivePaneLayout::Split {
                    split_id: 21,
                    direction: SplitDirection::Horizontal,
                    ratio_millis: 500,
                    ..
                }
            ) && matches!(second.as_ref(), LivePaneLayout::Pane { pane: 5 })
        ));
        assert_eq!(layout.panes(), [3, 4, 5]);

        layout.swap_panes(4, 5);
        assert_eq!(layout.panes(), [3, 5, 4]);
        assert!(layout.balance());
        assert!(!layout.balance());
        assert!(matches!(
            layout,
            LivePaneLayout::Split {
                ratio_millis: 500,
                ..
            }
        ));

        let mut next_split = 30;
        let shape = PaneLayoutPreset::Tall.shape(&[3, 4, 5, 6], 5).unwrap();
        let tall = LivePaneLayout::from_shape(shape, &mut next_split).unwrap();
        assert_eq!(next_split, 33);
        assert!(matches!(
            &tall,
            LivePaneLayout::Split {
                split_id: 30,
                direction: SplitDirection::Vertical,
                ratio_millis: 500,
                first,
                second,
            } if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 5 })
                && matches!(second.as_ref(), LivePaneLayout::Split {
                    split_id: 31,
                    direction: SplitDirection::Horizontal,
                    ratio_millis: 667,
                    ..
                })
        ));
        assert_eq!(tall.panes(), [5, 3, 4, 6]);

        let mut next_split = 40;
        let shape = PaneLayoutPreset::Fat.shape(&[3, 4, 5, 6], 4).unwrap();
        let fat = LivePaneLayout::from_shape(shape, &mut next_split).unwrap();
        assert!(matches!(
            &fat,
            LivePaneLayout::Split {
                direction: SplitDirection::Horizontal,
                ratio_millis: 500,
                first,
                second,
                ..
            } if matches!(first.as_ref(), LivePaneLayout::Pane { pane: 4 })
                && matches!(second.as_ref(), LivePaneLayout::Split {
                    direction: SplitDirection::Vertical,
                    ..
                })
        ));
        assert_eq!(fat.panes(), [4, 3, 5, 6]);
        let mut next_split = u32::MAX;
        assert!(
            LivePaneLayout::from_shape(
                PaneLayoutPreset::Rows.shape(&[3, 4], 3).unwrap(),
                &mut next_split
            )
            .unwrap_err()
            .to_string()
            .contains("exhausted")
        );
    }
}
