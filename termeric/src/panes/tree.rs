//! Pane tree structure for managing split layouts
//!
//! Uses a binary tree where each node is either a leaf (pane) or a split
//! containing two children.
use super::{Divider, DividerTarget, PANE_DIVIDER_THICKNESS};
use crate::ui::geometry::Bounds;
use std::collections::HashMap;
pub use termeric_core::split::SplitDirection;
use termeric_core::split::{LayoutShape, PaneLayoutPreset};

/// Unique identifier for a pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// A node in the pane tree
#[derive(Debug, Clone)]
pub enum PaneNode {
    /// A leaf node containing a single pane
    Leaf(PaneId),
    /// A split node containing two children
    Split {
        /// Monotonic tab-local identity. Runtime IDs are assigned when a tree
        /// is created or restored and survive unrelated subtree edits.
        local_split_id: u64,
        /// Daemon identity when this node came from a live-domain topology.
        /// Local/workspace split trees leave this unset.
        domain_split_id: Option<u32>,
        direction: SplitDirection,
        /// Ratio of first child (0.0 - 1.0)
        ratio: f32,
        first: Box<PaneNode>,
        second: Box<PaneNode>,
    },
}

impl PaneNode {
    /// Get all leaf pane IDs in this subtree
    pub fn get_leaves(&self) -> Vec<PaneId> {
        match self {
            PaneNode::Leaf(id) => vec![*id],
            PaneNode::Split { first, second, .. } => {
                let mut leaves = first.get_leaves();
                leaves.extend(second.get_leaves());
                leaves
            }
        }
    }

    /// Check if this node contains a specific pane
    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            PaneNode::Leaf(leaf_id) => *leaf_id == id,
            PaneNode::Split { first, second, .. } => first.contains(id) || second.contains(id),
        }
    }

    fn swap_leaves(&mut self, first_id: PaneId, second_id: PaneId) {
        match self {
            Self::Leaf(id) if *id == first_id => *id = second_id,
            Self::Leaf(id) if *id == second_id => *id = first_id,
            Self::Leaf(_) => {}
            Self::Split { first, second, .. } => {
                first.swap_leaves(first_id, second_id);
                second.swap_leaves(first_id, second_id);
            }
        }
    }

    fn balance(&mut self) {
        if let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = 0.5;
            first.balance();
            second.balance();
        }
    }

    /// The node for a preset's layout, numbering its splits from
    /// `next_split_id`; `None` when the numbers run out.
    fn from_shape(shape: LayoutShape<PaneId>, next_split_id: &mut u64) -> Option<Self> {
        match shape {
            LayoutShape::Pane(id) => Some(Self::Leaf(id)),
            LayoutShape::Split {
                direction,
                ratio_millis,
                first,
                second,
            } => {
                let local_split_id = *next_split_id;
                *next_split_id = next_split_id.checked_add(1)?;
                Some(Self::Split {
                    local_split_id,
                    domain_split_id: None,
                    direction,
                    ratio: f32::from(ratio_millis) / 1000.0,
                    first: Box::new(Self::from_shape(*first, next_split_id)?),
                    second: Box::new(Self::from_shape(*second, next_split_id)?),
                })
            }
        }
    }

    /// Find and split a leaf node
    fn split_leaf(
        &mut self,
        target: PaneId,
        new_id: PaneId,
        direction: SplitDirection,
        local_split_id: u64,
    ) -> bool {
        match self {
            PaneNode::Leaf(id) if *id == target => {
                // Replace this leaf with a split
                let old_node = Box::new(PaneNode::Leaf(*id));
                let new_node = Box::new(PaneNode::Leaf(new_id));
                *self = PaneNode::Split {
                    local_split_id,
                    domain_split_id: None,
                    direction,
                    ratio: 0.5,
                    first: old_node,
                    second: new_node,
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { first, second, .. } => {
                first.split_leaf(target, new_id, direction, local_split_id)
                    || second.split_leaf(target, new_id, direction, local_split_id)
            }
        }
    }

    fn assign_local_split_ids(&mut self, next_split_id: &mut u64) {
        if let Self::Split {
            local_split_id,
            first,
            second,
            ..
        } = self
        {
            *local_split_id = *next_split_id;
            *next_split_id = next_split_id
                .checked_add(1)
                .expect("bounded pane tree cannot exhaust u64 divider IDs during restore");
            first.assign_local_split_ids(next_split_id);
            second.assign_local_split_ids(next_split_id);
        }
    }

    /// Remove a leaf node and return the remaining sibling
    fn remove_leaf(&mut self, target: PaneId) -> Option<PaneNode> {
        match self {
            PaneNode::Leaf(id) if *id == target => {
                // This node should be removed by parent
                None
            }
            PaneNode::Leaf(_) => None,
            PaneNode::Split { first, second, .. } => {
                // Check if first child is the target
                if let PaneNode::Leaf(id) = first.as_ref() {
                    if *id == target {
                        // Return second child to replace this split
                        return Some(second.as_ref().clone());
                    }
                }

                // Check if second child is the target
                if let PaneNode::Leaf(id) = second.as_ref() {
                    if *id == target {
                        // Return first child to replace this split
                        return Some(first.as_ref().clone());
                    }
                }

                // Recursively try children
                if let Some(replacement) = first.remove_leaf(target) {
                    **first = replacement;
                    return None;
                }
                if let Some(replacement) = second.remove_leaf(target) {
                    **second = replacement;
                    return None;
                }
                None
            }
        }
    }

    /// Find the sibling of a pane
    fn find_sibling(&self, target: PaneId) -> Option<PaneId> {
        match self {
            PaneNode::Leaf(_) => None,
            PaneNode::Split { first, second, .. } => {
                // Check if target is first, return first leaf from second
                if let PaneNode::Leaf(id) = first.as_ref() {
                    if *id == target {
                        return second.get_leaves().first().copied();
                    }
                }

                // Check if target is second, return first leaf from first
                if let PaneNode::Leaf(id) = second.as_ref() {
                    if *id == target {
                        return first.get_leaves().first().copied();
                    }
                }

                // Recursively search
                first
                    .find_sibling(target)
                    .or_else(|| second.find_sibling(target))
            }
        }
    }

    /// Calculate layout for all panes in this subtree
    fn calculate_layout(&self, rect: Bounds, result: &mut HashMap<PaneId, Bounds>) {
        match self {
            PaneNode::Leaf(id) => {
                result.insert(*id, rect);
            }
            PaneNode::Split {
                direction,
                ratio,
                first,
                second,
                ..
            } => {
                let (first_rect, second_rect) = split_rects(rect, *direction, *ratio);
                first.calculate_layout(first_rect, result);
                second.calculate_layout(second_rect, result);
            }
        }
    }

    fn resize_target(
        &mut self,
        rect: Bounds,
        pos_x: f32,
        pos_y: f32,
        target: DividerTarget,
    ) -> Option<u16> {
        let PaneNode::Split {
            local_split_id,
            domain_split_id,
            direction,
            ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if divider_target(*local_split_id, *domain_split_id) == target {
            *ratio = match direction {
                SplitDirection::Horizontal => {
                    ((pos_y - rect.y) / (rect.height - PANE_DIVIDER_THICKNESS)).clamp(0.1, 0.9)
                }
                SplitDirection::Vertical => {
                    ((pos_x - rect.x) / (rect.width - PANE_DIVIDER_THICKNESS)).clamp(0.1, 0.9)
                }
            };
            return Some((*ratio * 1_000.0).round() as u16);
        }
        let (first_rect, second_rect) = split_rects(rect, *direction, *ratio);
        first
            .resize_target(first_rect, pos_x, pos_y, target)
            .or_else(|| second.resize_target(second_rect, pos_x, pos_y, target))
    }

    fn set_target_ratio(&mut self, target: DividerTarget, ratio_millis: u16) -> Option<u16> {
        let Self::Split {
            local_split_id,
            domain_split_id,
            ratio,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };
        if divider_target(*local_split_id, *domain_split_id) == target {
            let previous = (*ratio * 1_000.0).round() as u16;
            *ratio = f32::from(ratio_millis) / 1_000.0;
            return Some(previous);
        }
        first
            .set_target_ratio(target, ratio_millis)
            .or_else(|| second.set_target_ratio(target, ratio_millis))
    }

    /// Get dividers for rendering
    fn get_dividers(&self, rect: Bounds, result: &mut Vec<Divider>) {
        let PaneNode::Split {
            local_split_id,
            domain_split_id,
            direction,
            ratio,
            first,
            second,
        } = self
        else {
            return;
        };
        let (first_rect, second_rect) = split_rects(rect, *direction, *ratio);
        let (start, end, position, vertical) = match direction {
            SplitDirection::Horizontal => (
                rect.x,
                rect.x + rect.width,
                first_rect.y + first_rect.height,
                false,
            ),
            SplitDirection::Vertical => (
                rect.y,
                rect.y + rect.height,
                first_rect.x + first_rect.width,
                true,
            ),
        };
        result.push(Divider {
            start,
            end,
            position,
            vertical,
            ratio_millis: (*ratio * 1_000.0).round() as u16,
            target: divider_target(*local_split_id, *domain_split_id),
        });
        first.get_dividers(first_rect, result);
        second.get_dividers(second_rect, result);
    }
}

/// The rectangles of a split's two children, either side of its divider.
fn split_rects(rect: Bounds, direction: SplitDirection, ratio: f32) -> (Bounds, Bounds) {
    match direction {
        SplitDirection::Horizontal => {
            let first_height = (rect.height - PANE_DIVIDER_THICKNESS) * ratio;
            (
                Bounds {
                    height: first_height,
                    ..rect
                },
                Bounds {
                    y: rect.y + first_height + PANE_DIVIDER_THICKNESS,
                    height: rect.height - first_height - PANE_DIVIDER_THICKNESS,
                    ..rect
                },
            )
        }
        SplitDirection::Vertical => {
            let first_width = (rect.width - PANE_DIVIDER_THICKNESS) * ratio;
            (
                Bounds {
                    width: first_width,
                    ..rect
                },
                Bounds {
                    x: rect.x + first_width + PANE_DIVIDER_THICKNESS,
                    width: rect.width - first_width - PANE_DIVIDER_THICKNESS,
                    ..rect
                },
            )
        }
    }
}

/// A split's divider identity: its daemon ID in a live domain, otherwise its
/// tab-local one.
fn divider_target(local_split_id: u64, domain_split_id: Option<u32>) -> DividerTarget {
    domain_split_id
        .map(DividerTarget::Domain)
        .unwrap_or(DividerTarget::Local(local_split_id))
}

/// The pane tree structure
#[derive(Debug)]
pub struct PaneTree {
    root: PaneNode,
    next_local_split_id: u64,
}

impl PaneTree {
    /// A tree of one pane.
    pub fn new(id: PaneId) -> Self {
        Self {
            root: PaneNode::Leaf(id),
            next_local_split_id: 1,
        }
    }

    #[cfg(test)]
    pub(super) fn set_next_local_split_id(&mut self, next_local_split_id: u64) {
        self.next_local_split_id = next_local_split_id;
    }

    pub(crate) fn root(&self) -> &PaneNode {
        &self.root
    }

    pub(crate) fn has_split_capacity(&self) -> bool {
        self.next_local_split_id.checked_add(1).is_some()
    }

    pub(crate) fn from_root(mut root: PaneNode) -> Self {
        let mut next_local_split_id = 1;
        root.assign_local_split_ids(&mut next_local_split_id);
        Self {
            root,
            next_local_split_id,
        }
    }

    /// Split a pane
    pub fn split(&mut self, target: PaneId, new_id: PaneId, direction: SplitDirection) -> bool {
        let Some(next) = self.next_local_split_id.checked_add(1) else {
            return false;
        };
        if !self
            .root
            .split_leaf(target, new_id, direction, self.next_local_split_id)
        {
            return false;
        }
        self.next_local_split_id = next;
        true
    }

    /// Remove a pane
    pub fn remove(&mut self, target: PaneId) {
        if let Some(replacement) = self.root.remove_leaf(target) {
            self.root = replacement;
        }
    }

    /// Find the sibling of a pane
    pub fn find_sibling(&self, id: PaneId) -> Option<PaneId> {
        self.root.find_sibling(id)
    }

    /// Calculate layout for all panes
    pub fn calculate_layout(&self, rect: Bounds) -> HashMap<PaneId, Bounds> {
        let mut result = HashMap::new();
        self.root.calculate_layout(rect, &mut result);
        result
    }

    /// Get dividers for rendering
    pub fn get_dividers(&self, rect: Bounds) -> Vec<Divider> {
        let mut result = Vec::new();
        self.root.get_dividers(rect, &mut result);
        result
    }

    /// Get all pane IDs
    pub fn get_all_ids(&self) -> Vec<PaneId> {
        self.root.get_leaves()
    }

    /// Exchange two pane identities while preserving the split geometry.
    pub fn swap(&mut self, first: PaneId, second: PaneId) -> bool {
        if first == second || !self.root.contains(first) || !self.root.contains(second) {
            return false;
        }
        self.root.swap_leaves(first, second);
        true
    }

    /// Reset every divider in the current layout to an even ratio.
    pub fn balance(&mut self) {
        self.root.balance();
    }

    /// Rebuild the tree from the same stable pane IDs in a preset's
    /// layout, `focused` being the master of Tall and Fat. False, leaving
    /// the tree as it was, when the preset does not apply or split
    /// identities run out.
    pub fn apply_layout_preset(&mut self, preset: PaneLayoutPreset, focused: PaneId) -> bool {
        let mut next = self.next_local_split_id;
        let Some(root) = preset
            .shape(&self.get_all_ids(), focused)
            .and_then(|shape| PaneNode::from_shape(shape, &mut next))
        else {
            return false;
        };
        self.root = root;
        self.next_local_split_id = next;
        true
    }

    pub fn resize_target(
        &mut self,
        rect: Bounds,
        pos_x: f32,
        pos_y: f32,
        target: DividerTarget,
    ) -> Option<u16> {
        self.root.resize_target(rect, pos_x, pos_y, target)
    }

    pub fn set_target_ratio(&mut self, target: DividerTarget, ratio_millis: u16) -> Option<u16> {
        self.root.set_target_ratio(target, ratio_millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn next_property_value(state: &mut u64) -> u64 {
        // A fixed xorshift sequence keeps this property harness reproducible
        // across platforms and makes a failing seed straightforward to replay.
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn assert_balanced(node: &PaneNode) {
        if let PaneNode::Split {
            ratio,
            first,
            second,
            ..
        } = node
        {
            assert_eq!(*ratio, 0.5);
            assert_balanced(first);
            assert_balanced(second);
        }
    }

    fn assert_tree_invariants(
        tree: &PaneTree,
        expected: &BTreeSet<u32>,
        bounds: Bounds,
        seed: u64,
        step: usize,
    ) {
        const GEOMETRY_EPSILON: f32 = 0.01;
        let ids = tree.get_all_ids();
        let actual: BTreeSet<_> = ids.iter().map(|id| id.0).collect();
        assert_eq!(
            actual, *expected,
            "leaf set diverged for seed {seed} at step {step}"
        );
        assert_eq!(
            ids.len(),
            actual.len(),
            "duplicate leaf for seed {seed} at step {step}"
        );

        let layout = tree.calculate_layout(bounds);
        assert_eq!(layout.len(), expected.len());
        assert_eq!(tree.get_dividers(bounds).len(), expected.len() - 1);
        for id in ids {
            let rect = layout
                .get(&id)
                .unwrap_or_else(|| panic!("missing layout for {id:?}, seed {seed}, step {step}"));
            for value in [rect.x, rect.y, rect.width, rect.height] {
                assert!(
                    value.is_finite(),
                    "non-finite geometry for seed {seed} at step {step}: {rect:?}"
                );
            }
            assert!(rect.width >= 0.0 && rect.height >= 0.0);
            assert!(rect.x >= bounds.x - GEOMETRY_EPSILON);
            assert!(rect.y >= bounds.y - GEOMETRY_EPSILON);
            assert!(
                rect.x + rect.width <= bounds.x + bounds.width + GEOMETRY_EPSILON,
                "horizontal overflow for seed {seed} at step {step}: {rect:?}"
            );
            assert!(
                rect.y + rect.height <= bounds.y + bounds.height + GEOMETRY_EPSILON,
                "vertical overflow for seed {seed} at step {step}: {rect:?}"
            );
            if expected.len() > 1 {
                let sibling = tree.find_sibling(id).unwrap_or_else(|| {
                    panic!("missing sibling for {id:?}, seed {seed}, step {step}")
                });
                assert_ne!(sibling, id);
                assert!(expected.contains(&sibling.0));
            }
        }
    }

    #[test]
    fn test_single_pane() {
        let tree = PaneTree::new(PaneId(0));
        let layout = tree.calculate_layout(Bounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        });

        assert_eq!(layout.len(), 1);
        let rect = layout.get(&PaneId(0)).unwrap();
        assert_eq!(rect.width, 800.0);
        assert_eq!(rect.height, 600.0);
    }

    #[test]
    fn local_divider_ids_survive_unrelated_collapse_and_stale_ids_fail_exactly() {
        let bounds = Bounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        let mut tree = PaneTree::new(PaneId(0));
        assert!(tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical));
        assert!(tree.split(PaneId(1), PaneId(2), SplitDirection::Horizontal));
        let dividers = tree.get_dividers(bounds);
        let root_id = match dividers[0].target {
            DividerTarget::Local(id) => id,
            DividerTarget::Domain(_) => panic!("ordinary tree returned a domain divider"),
        };
        let nested_id = match dividers[1].target {
            DividerTarget::Local(id) => id,
            DividerTarget::Domain(_) => panic!("ordinary tree returned a domain divider"),
        };
        assert_ne!(root_id, nested_id);
        assert_eq!(
            tree.set_target_ratio(DividerTarget::Local(nested_id), 700),
            Some(500)
        );
        assert_eq!(
            tree.set_target_ratio(DividerTarget::Local(u64::MAX), 600),
            None
        );

        tree.remove(PaneId(0));
        let surviving = tree.get_dividers(bounds);
        assert_eq!(surviving.len(), 1);
        assert_eq!(surviving[0].target, DividerTarget::Local(nested_id));
        assert_eq!(surviving[0].ratio_millis, 700);

        assert!(tree.apply_layout_preset(PaneLayoutPreset::Columns, PaneId(1)));
        let replacement = tree.get_dividers(bounds);
        let replacement_id = match replacement[0].target {
            DividerTarget::Local(id) => id,
            DividerTarget::Domain(_) => panic!("ordinary preset returned a domain divider"),
        };
        assert!(replacement_id > nested_id);
    }

    #[test]
    fn test_vertical_split() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);

        let layout = tree.calculate_layout(Bounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        });
        assert_eq!(layout.len(), 2);

        let rect0 = layout.get(&PaneId(0)).unwrap();
        let rect1 = layout.get(&PaneId(1)).unwrap();

        // First pane should be on the left
        assert!(rect0.x < rect1.x);
        // Both should have same height
        assert_eq!(rect0.height, rect1.height);
    }

    #[test]
    fn test_horizontal_split() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Horizontal);

        let layout = tree.calculate_layout(Bounds {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        });
        assert_eq!(layout.len(), 2);

        let rect0 = layout.get(&PaneId(0)).unwrap();
        let rect1 = layout.get(&PaneId(1)).unwrap();

        // First pane should be on top
        assert!(rect0.y < rect1.y);
        // Both should have same width
        assert_eq!(rect0.width, rect1.width);
    }

    #[test]
    fn test_find_sibling() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);

        assert_eq!(tree.find_sibling(PaneId(0)), Some(PaneId(1)));
        assert_eq!(tree.find_sibling(PaneId(1)), Some(PaneId(0)));
    }

    #[test]
    fn test_remove_pane() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);
        tree.remove(PaneId(1));

        let ids = tree.get_all_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], PaneId(0));
    }

    #[test]
    fn swap_balance_and_presets_preserve_the_leaf_set() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);
        tree.split(PaneId(1), PaneId(2), SplitDirection::Horizontal);
        let bounds = Bounds::new(0.0, 0.0, 900.0, 600.0);
        let root = tree
            .get_dividers(bounds)
            .into_iter()
            .find(|divider| divider.vertical)
            .unwrap();
        assert!(
            tree.resize_target(bounds, 270.0, 100.0, root.target)
                .is_some()
        );

        let before = tree.calculate_layout(bounds);
        assert!(tree.swap(PaneId(0), PaneId(2)));
        let after = tree.calculate_layout(bounds);
        let geometry = |rect: Bounds| (rect.x, rect.y, rect.width, rect.height);
        assert_eq!(geometry(before[&PaneId(0)]), geometry(after[&PaneId(2)]));
        assert_eq!(geometry(before[&PaneId(2)]), geometry(after[&PaneId(0)]));

        tree.balance();
        let balanced = tree.calculate_layout(bounds);
        assert!((balanced[&PaneId(2)].width - 448.0).abs() < 0.1);

        for preset in [
            PaneLayoutPreset::Columns,
            PaneLayoutPreset::Rows,
            PaneLayoutPreset::Grid,
            PaneLayoutPreset::Tall,
            PaneLayoutPreset::Fat,
        ] {
            assert!(tree.apply_layout_preset(preset, PaneId(2)));
            let mut ids = tree.get_all_ids();
            ids.sort_by_key(|id| id.0);
            assert_eq!(ids, vec![PaneId(0), PaneId(1), PaneId(2)]);
            assert_eq!(tree.calculate_layout(bounds).len(), 3);
        }
    }

    #[test]
    fn generated_tree_operations_preserve_identity_geometry_and_balance_invariants() {
        const SEEDS: u64 = 128;
        const STEPS: usize = 96;
        const MAX_PANES: usize = 16;
        let bounds = Bounds::new(11.0, 17.0, 4096.0, 3072.0);

        for seed in 1..=SEEDS {
            let mut state = seed;
            let mut tree = PaneTree::new(PaneId(0));
            let mut expected = BTreeSet::from([0]);
            let mut next_id = 1_u32;

            for step in 0..STEPS {
                let value = next_property_value(&mut state);
                let ids = tree.get_all_ids();
                match value % 5 {
                    0 if ids.len() < MAX_PANES => {
                        let target = ids[(value as usize >> 8) % ids.len()];
                        let direction = if value & 1 == 0 {
                            SplitDirection::Horizontal
                        } else {
                            SplitDirection::Vertical
                        };
                        assert!(tree.split(target, PaneId(next_id), direction));
                        assert!(expected.insert(next_id));
                        next_id += 1;
                    }
                    1 if ids.len() > 1 => {
                        let first_index = (value as usize >> 8) % ids.len();
                        let second_index = (first_index + 1 + (value as usize >> 16)) % ids.len();
                        let first = ids[first_index];
                        let second = ids[second_index];
                        if first != second {
                            let before = tree.calculate_layout(bounds);
                            assert!(tree.swap(first, second));
                            let after = tree.calculate_layout(bounds);
                            let geometry = |rect: Bounds| (rect.x, rect.y, rect.width, rect.height);
                            assert_eq!(geometry(after[&first]), geometry(before[&second]));
                            assert_eq!(geometry(after[&second]), geometry(before[&first]));
                        }
                    }
                    2 => {
                        tree.balance();
                        assert_balanced(tree.root());
                    }
                    3 => {
                        let preset = [
                            PaneLayoutPreset::Columns,
                            PaneLayoutPreset::Rows,
                            PaneLayoutPreset::Grid,
                            PaneLayoutPreset::Tall,
                            PaneLayoutPreset::Fat,
                        ][(value as usize >> 8) % 5];
                        let focused = ids[(value as usize >> 16) % ids.len()];
                        assert_eq!(tree.apply_layout_preset(preset, focused), ids.len() > 1);
                    }
                    4 if ids.len() > 1 => {
                        let removed = ids[(value as usize >> 8) % ids.len()];
                        tree.remove(removed);
                        assert!(expected.remove(&removed.0));
                    }
                    _ => {}
                }
                assert_tree_invariants(&tree, &expected, bounds, seed, step);
            }
        }
    }

    #[test]
    fn exact_nested_divider_target_never_jumps_to_the_root_during_drag() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);
        tree.split(PaneId(1), PaneId(2), SplitDirection::Horizontal);
        let bounds = Bounds::new(0.0, 0.0, 800.0, 600.0);
        let before = tree.get_dividers(bounds);
        let root = before.iter().find(|divider| divider.vertical).unwrap();
        let nested = before.iter().find(|divider| !divider.vertical).unwrap();
        let root_position = root.position;
        let nested_position = nested.position;

        let ratio = tree
            .resize_target(bounds, 600.0, 450.0, nested.target)
            .unwrap();
        assert!(termeric_core::split::RATIO_MILLIS.contains(&ratio));
        let after = tree.get_dividers(bounds);
        assert_eq!(
            after
                .iter()
                .find(|divider| divider.vertical)
                .unwrap()
                .position,
            root_position
        );
        assert_ne!(
            after
                .iter()
                .find(|divider| !divider.vertical)
                .unwrap()
                .position,
            nested_position
        );
    }
}
