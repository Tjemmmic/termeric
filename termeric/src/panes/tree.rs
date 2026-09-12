//! Pane tree structure for managing split layouts
//!
//! Uses a binary tree where each node is either a leaf (pane) or a split
//! containing two children.

use super::layout::PaneRect;
use super::{Divider, DividerTarget, PANE_DIVIDER_HIT_MARGIN, PANE_DIVIDER_THICKNESS};
use std::collections::HashMap;

/// Unique identifier for a pane
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub u32);

/// Direction of a split
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Split horizontally (panes stacked vertically)
    Horizontal,
    /// Split vertically (panes side by side)
    Vertical,
}

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

    fn preset(
        ids: &[PaneId],
        direction: SplitDirection,
        alternate: bool,
        next_split_id: &mut u64,
    ) -> Option<Self> {
        debug_assert!(!ids.is_empty());
        if ids.len() == 1 {
            return Some(Self::Leaf(ids[0]));
        }
        let local_split_id = *next_split_id;
        *next_split_id = next_split_id.checked_add(1)?;
        let midpoint = ids.len().div_ceil(2);
        let next_direction = if alternate {
            match direction {
                SplitDirection::Horizontal => SplitDirection::Vertical,
                SplitDirection::Vertical => SplitDirection::Horizontal,
            }
        } else {
            direction
        };
        Some(Self::Split {
            local_split_id,
            domain_split_id: None,
            direction,
            ratio: midpoint as f32 / ids.len() as f32,
            first: Box::new(Self::preset(
                &ids[..midpoint],
                next_direction,
                alternate,
                next_split_id,
            )?),
            second: Box::new(Self::preset(
                &ids[midpoint..],
                next_direction,
                alternate,
                next_split_id,
            )?),
        })
    }

    fn master_detail_preset(
        ids: &[PaneId],
        master: PaneId,
        direction: SplitDirection,
        next_split_id: &mut u64,
    ) -> Option<Self> {
        debug_assert!(ids.len() >= 2);
        debug_assert!(ids.contains(&master));
        let detail_ids = ids
            .iter()
            .copied()
            .filter(|id| *id != master)
            .collect::<Vec<_>>();
        let detail_direction = match direction {
            SplitDirection::Horizontal => SplitDirection::Vertical,
            SplitDirection::Vertical => SplitDirection::Horizontal,
        };
        let local_split_id = *next_split_id;
        *next_split_id = next_split_id.checked_add(1)?;
        Some(Self::Split {
            local_split_id,
            domain_split_id: None,
            direction,
            ratio: 0.5,
            first: Box::new(Self::Leaf(master)),
            second: Box::new(Self::preset(
                &detail_ids,
                detail_direction,
                false,
                next_split_id,
            )?),
        })
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
    fn calculate_layout(&self, rect: PaneRect, result: &mut HashMap<PaneId, PaneRect>) {
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
                let (first_rect, second_rect) = match direction {
                    SplitDirection::Horizontal => {
                        // Split horizontally (top/bottom)
                        let divider_height: f32 = 4.0; // Divider thickness
                        let first_height = (rect.height - divider_height) * ratio;
                        let second_height = rect.height - first_height - divider_height;
                        (
                            PaneRect {
                                x: rect.x,
                                y: rect.y,
                                width: rect.width,
                                height: first_height,
                            },
                            PaneRect {
                                x: rect.x,
                                y: rect.y + first_height + divider_height,
                                width: rect.width,
                                height: second_height,
                            },
                        )
                    }
                    SplitDirection::Vertical => {
                        // Split vertically (left/right)
                        let divider_width: f32 = 4.0; // Divider thickness
                        let first_width = (rect.width - divider_width) * ratio;
                        let second_width = rect.width - first_width - divider_width;
                        (
                            PaneRect {
                                x: rect.x,
                                y: rect.y,
                                width: first_width,
                                height: rect.height,
                            },
                            PaneRect {
                                x: rect.x + first_width + divider_width,
                                y: rect.y,
                                width: second_width,
                                height: rect.height,
                            },
                        )
                    }
                };

                first.calculate_layout(first_rect, result);
                second.calculate_layout(second_rect, result);
            }
        }
    }

    /// Resize a split node whose divider is near the given position.
    ///
    /// `pos` is the mouse position in the divider's axis (x for vertical, y for horizontal).
    /// Returns true if a split was resized.
    fn resize_at(&mut self, rect: PaneRect, pos_x: f32, pos_y: f32, hit_threshold: f32) -> bool {
        if let PaneNode::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } = self
        {
            let divider_thickness = PANE_DIVIDER_THICKNESS;
            match direction {
                SplitDirection::Horizontal => {
                    let divider_y = rect.y + (rect.height - divider_thickness) * *ratio;
                    if (pos_y - divider_y).abs() < hit_threshold
                        && pos_x >= rect.x
                        && pos_x <= rect.x + rect.width
                    {
                        let new_ratio =
                            ((pos_y - rect.y) / (rect.height - divider_thickness)).clamp(0.1, 0.9);
                        *ratio = new_ratio;
                        return true;
                    }
                    // Recurse into children
                    let first_height = (rect.height - divider_thickness) * *ratio;
                    if first.resize_at(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_height,
                        },
                        pos_x,
                        pos_y,
                        hit_threshold,
                    ) {
                        return true;
                    }
                    if second.resize_at(
                        PaneRect {
                            x: rect.x,
                            y: rect.y + first_height + divider_thickness,
                            width: rect.width,
                            height: rect.height - first_height - divider_thickness,
                        },
                        pos_x,
                        pos_y,
                        hit_threshold,
                    ) {
                        return true;
                    }
                }
                SplitDirection::Vertical => {
                    let divider_x = rect.x + (rect.width - divider_thickness) * *ratio;
                    if (pos_x - divider_x).abs() < hit_threshold
                        && pos_y >= rect.y
                        && pos_y <= rect.y + rect.height
                    {
                        let new_ratio =
                            ((pos_x - rect.x) / (rect.width - divider_thickness)).clamp(0.1, 0.9);
                        *ratio = new_ratio;
                        return true;
                    }
                    // Recurse into children
                    let first_width = (rect.width - divider_thickness) * *ratio;
                    if first.resize_at(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: first_width,
                            height: rect.height,
                        },
                        pos_x,
                        pos_y,
                        hit_threshold,
                    ) {
                        return true;
                    }
                    if second.resize_at(
                        PaneRect {
                            x: rect.x + first_width + divider_thickness,
                            y: rect.y,
                            width: rect.width - first_width - divider_thickness,
                            height: rect.height,
                        },
                        pos_x,
                        pos_y,
                        hit_threshold,
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn resize_target(
        &mut self,
        rect: PaneRect,
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
        let own_target = domain_split_id
            .map(DividerTarget::Domain)
            .unwrap_or(DividerTarget::Local(*local_split_id));
        let divider_thickness = PANE_DIVIDER_THICKNESS;
        if own_target == target {
            *ratio = match direction {
                SplitDirection::Horizontal => {
                    ((pos_y - rect.y) / (rect.height - divider_thickness)).clamp(0.1, 0.9)
                }
                SplitDirection::Vertical => {
                    ((pos_x - rect.x) / (rect.width - divider_thickness)).clamp(0.1, 0.9)
                }
            };
            return Some((*ratio * 1_000.0).round() as u16);
        }

        match direction {
            SplitDirection::Horizontal => {
                let first_height = (rect.height - divider_thickness) * *ratio;
                first
                    .resize_target(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_height,
                        },
                        pos_x,
                        pos_y,
                        target,
                    )
                    .or_else(|| {
                        second.resize_target(
                            PaneRect {
                                x: rect.x,
                                y: rect.y + first_height + divider_thickness,
                                width: rect.width,
                                height: rect.height - first_height - divider_thickness,
                            },
                            pos_x,
                            pos_y,
                            target,
                        )
                    })
            }
            SplitDirection::Vertical => {
                let first_width = (rect.width - divider_thickness) * *ratio;
                first
                    .resize_target(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: first_width,
                            height: rect.height,
                        },
                        pos_x,
                        pos_y,
                        target,
                    )
                    .or_else(|| {
                        second.resize_target(
                            PaneRect {
                                x: rect.x + first_width + divider_thickness,
                                y: rect.y,
                                width: rect.width - first_width - divider_thickness,
                                height: rect.height,
                            },
                            pos_x,
                            pos_y,
                            target,
                        )
                    })
            }
        }
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
        let own_target = domain_split_id
            .map(DividerTarget::Domain)
            .unwrap_or(DividerTarget::Local(*local_split_id));
        if own_target == target {
            let previous = (*ratio * 1_000.0).round() as u16;
            *ratio = f32::from(ratio_millis) / 1_000.0;
            return Some(previous);
        }
        first
            .set_target_ratio(target, ratio_millis)
            .or_else(|| second.set_target_ratio(target, ratio_millis))
    }

    /// Check if a position is near a divider (for cursor change / drag detection)
    fn hit_test_divider(
        &self,
        rect: PaneRect,
        pos_x: f32,
        pos_y: f32,
        threshold: f32,
    ) -> Option<bool> {
        if let PaneNode::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } = self
        {
            let divider_thickness = PANE_DIVIDER_THICKNESS;
            match direction {
                SplitDirection::Horizontal => {
                    let divider_y = rect.y + (rect.height - divider_thickness) * *ratio;
                    if (pos_y - divider_y).abs() < threshold
                        && pos_x >= rect.x
                        && pos_x <= rect.x + rect.width
                    {
                        return Some(false); // horizontal divider → vertical resize cursor
                    }
                    let first_height = (rect.height - divider_thickness) * *ratio;
                    if let Some(v) = first.hit_test_divider(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_height,
                        },
                        pos_x,
                        pos_y,
                        threshold,
                    ) {
                        return Some(v);
                    }
                    if let Some(v) = second.hit_test_divider(
                        PaneRect {
                            x: rect.x,
                            y: rect.y + first_height + divider_thickness,
                            width: rect.width,
                            height: rect.height - first_height - divider_thickness,
                        },
                        pos_x,
                        pos_y,
                        threshold,
                    ) {
                        return Some(v);
                    }
                }
                SplitDirection::Vertical => {
                    let divider_x = rect.x + (rect.width - divider_thickness) * *ratio;
                    if (pos_x - divider_x).abs() < threshold
                        && pos_y >= rect.y
                        && pos_y <= rect.y + rect.height
                    {
                        return Some(true); // vertical divider → horizontal resize cursor
                    }
                    let first_width = (rect.width - divider_thickness) * *ratio;
                    if let Some(v) = first.hit_test_divider(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: first_width,
                            height: rect.height,
                        },
                        pos_x,
                        pos_y,
                        threshold,
                    ) {
                        return Some(v);
                    }
                    if let Some(v) = second.hit_test_divider(
                        PaneRect {
                            x: rect.x + first_width + divider_thickness,
                            y: rect.y,
                            width: rect.width - first_width - divider_thickness,
                            height: rect.height,
                        },
                        pos_x,
                        pos_y,
                        threshold,
                    ) {
                        return Some(v);
                    }
                }
            }
        }
        None
    }

    /// Get dividers for rendering
    fn get_dividers(&self, rect: PaneRect, result: &mut Vec<Divider>) {
        if let PaneNode::Split {
            local_split_id,
            domain_split_id,
            direction,
            ratio,
            first,
            second,
        } = self
        {
            let divider_thickness = PANE_DIVIDER_THICKNESS;
            match direction {
                SplitDirection::Horizontal => {
                    let divider_y = rect.y + (rect.height - divider_thickness) * ratio;
                    result.push(Divider {
                        start: rect.x,
                        end: rect.x + rect.width,
                        position: divider_y,
                        vertical: false,
                        ratio_millis: (*ratio * 1_000.0).round() as u16,
                        target: domain_split_id
                            .map(DividerTarget::Domain)
                            .unwrap_or(DividerTarget::Local(*local_split_id)),
                    });

                    // Recurse into children
                    let first_height = (rect.height - divider_thickness) * ratio;
                    first.get_dividers(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_height,
                        },
                        result,
                    );
                    second.get_dividers(
                        PaneRect {
                            x: rect.x,
                            y: rect.y + first_height + divider_thickness,
                            width: rect.width,
                            height: rect.height - first_height - divider_thickness,
                        },
                        result,
                    );
                }
                SplitDirection::Vertical => {
                    let divider_x = rect.x + (rect.width - divider_thickness) * ratio;
                    result.push(Divider {
                        start: rect.y,
                        end: rect.y + rect.height,
                        position: divider_x,
                        vertical: true,
                        ratio_millis: (*ratio * 1_000.0).round() as u16,
                        target: domain_split_id
                            .map(DividerTarget::Domain)
                            .unwrap_or(DividerTarget::Local(*local_split_id)),
                    });

                    // Recurse into children
                    let first_width = (rect.width - divider_thickness) * ratio;
                    first.get_dividers(
                        PaneRect {
                            x: rect.x,
                            y: rect.y,
                            width: first_width,
                            height: rect.height,
                        },
                        result,
                    );
                    second.get_dividers(
                        PaneRect {
                            x: rect.x + first_width + divider_thickness,
                            y: rect.y,
                            width: rect.width - first_width - divider_thickness,
                            height: rect.height,
                        },
                        result,
                    );
                }
            }
        }
    }
}

/// The pane tree structure
#[derive(Debug)]
pub struct PaneTree {
    root: PaneNode,
    next_local_split_id: u64,
}

impl PaneTree {
    /// Create a new tree with a single pane
    pub fn new(id: PaneId) -> Self {
        Self {
            root: PaneNode::Leaf(id),
            next_local_split_id: 1,
        }
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
    pub fn calculate_layout(&self, rect: PaneRect) -> HashMap<PaneId, PaneRect> {
        let mut result = HashMap::new();
        self.root.calculate_layout(rect, &mut result);
        result
    }

    /// Get dividers for rendering
    pub fn get_dividers(&self, rect: PaneRect) -> Vec<Divider> {
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

    /// Rebuild the tree from the same stable pane IDs.
    pub fn apply_preset(&mut self, direction: SplitDirection, alternate: bool) -> bool {
        let ids = self.get_all_ids();
        let mut next = self.next_local_split_id;
        let Some(root) = PaneNode::preset(&ids, direction, alternate, &mut next) else {
            return false;
        };
        self.root = root;
        self.next_local_split_id = next;
        true
    }

    /// Rebuild the tree with one focused master and evenly tiled details.
    pub fn apply_master_detail_preset(
        &mut self,
        master: PaneId,
        direction: SplitDirection,
    ) -> bool {
        let ids = self.get_all_ids();
        if ids.len() < 2 || !ids.contains(&master) {
            return false;
        }
        let mut next = self.next_local_split_id;
        let Some(root) = PaneNode::master_detail_preset(&ids, master, direction, &mut next) else {
            return false;
        };
        self.root = root;
        self.next_local_split_id = next;
        true
    }

    /// Resize a split at the given position. Returns true if a divider was resized.
    ///
    /// `threshold` controls how close the mouse must be to a divider to match.
    /// Use a small value (8.0) for initial click detection and a large value
    /// (e.g. `f32::MAX`) during active drag so the divider tracks the mouse.
    pub fn resize_at(&mut self, rect: PaneRect, pos_x: f32, pos_y: f32, threshold: f32) -> bool {
        self.root.resize_at(rect, pos_x, pos_y, threshold)
    }

    pub fn resize_target(
        &mut self,
        rect: PaneRect,
        pos_x: f32,
        pos_y: f32,
        target: DividerTarget,
    ) -> Option<u16> {
        self.root.resize_target(rect, pos_x, pos_y, target)
    }

    pub fn set_target_ratio(&mut self, target: DividerTarget, ratio_millis: u16) -> Option<u16> {
        self.root.set_target_ratio(target, ratio_millis)
    }

    /// Hit-test a divider. Returns Some(is_vertical) if near a divider, None otherwise.
    pub fn hit_test_divider(&self, rect: PaneRect, pos_x: f32, pos_y: f32) -> Option<bool> {
        self.root
            .hit_test_divider(rect, pos_x, pos_y, PANE_DIVIDER_HIT_MARGIN)
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
        bounds: PaneRect,
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
        let layout = tree.calculate_layout(PaneRect {
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
        let bounds = PaneRect {
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

        assert!(tree.apply_preset(SplitDirection::Vertical, false));
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

        let layout = tree.calculate_layout(PaneRect {
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

        let layout = tree.calculate_layout(PaneRect {
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
        let bounds = PaneRect::new(0.0, 0.0, 900.0, 600.0);
        assert!(tree.resize_at(bounds, 270.0, 100.0, f32::MAX));

        let before = tree.calculate_layout(bounds);
        assert!(tree.swap(PaneId(0), PaneId(2)));
        let after = tree.calculate_layout(bounds);
        let geometry = |rect: PaneRect| (rect.x, rect.y, rect.width, rect.height);
        assert_eq!(geometry(before[&PaneId(0)]), geometry(after[&PaneId(2)]));
        assert_eq!(geometry(before[&PaneId(2)]), geometry(after[&PaneId(0)]));

        tree.balance();
        let balanced = tree.calculate_layout(bounds);
        assert!((balanced[&PaneId(2)].width - 448.0).abs() < 0.1);

        for (direction, alternate) in [
            (SplitDirection::Vertical, false),
            (SplitDirection::Horizontal, false),
            (SplitDirection::Vertical, true),
        ] {
            tree.apply_preset(direction, alternate);
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
        let bounds = PaneRect::new(11.0, 17.0, 4096.0, 3072.0);

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
                            let geometry =
                                |rect: PaneRect| (rect.x, rect.y, rect.width, rect.height);
                            assert_eq!(geometry(after[&first]), geometry(before[&second]));
                            assert_eq!(geometry(after[&second]), geometry(before[&first]));
                        }
                    }
                    2 => {
                        tree.balance();
                        assert_balanced(tree.root());
                    }
                    3 => {
                        let direction = if value & 1 == 0 {
                            SplitDirection::Horizontal
                        } else {
                            SplitDirection::Vertical
                        };
                        tree.apply_preset(direction, value & 2 != 0);
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
    fn asymmetric_divider_render_data_and_hit_tests_share_geometry_across_scales() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);
        tree.split(PaneId(1), PaneId(2), SplitDirection::Horizontal);
        let base = PaneRect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
        assert!(tree.resize_at(base, 280.0, 100.0, f32::MAX));

        for scale in [0.75_f32, 1.0, 2.0] {
            let bounds = PaneRect {
                x: 13.0 * scale,
                y: 17.0 * scale,
                width: 800.0 * scale,
                height: 600.0 * scale,
            };
            let dividers = tree.get_dividers(bounds);
            assert_eq!(dividers.len(), 2);
            for divider in dividers {
                let along = (divider.start + divider.end) / 2.0;
                let across = divider.position + PANE_DIVIDER_THICKNESS / 2.0;
                let hit = if divider.vertical {
                    tree.hit_test_divider(bounds, across, along)
                } else {
                    tree.hit_test_divider(bounds, along, across)
                };
                assert_eq!(hit, Some(divider.vertical));
            }
        }
    }

    #[test]
    fn exact_nested_divider_target_never_jumps_to_the_root_during_drag() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);
        tree.split(PaneId(1), PaneId(2), SplitDirection::Horizontal);
        let bounds = PaneRect::new(0.0, 0.0, 800.0, 600.0);
        let before = tree.get_dividers(bounds);
        let root = before.iter().find(|divider| divider.vertical).unwrap();
        let nested = before.iter().find(|divider| !divider.vertical).unwrap();
        let root_position = root.position;
        let nested_position = nested.position;

        let ratio = tree
            .resize_target(bounds, 600.0, 450.0, nested.target)
            .unwrap();
        assert!((100..=900).contains(&ratio));
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
