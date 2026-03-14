//! Pane tree structure for managing split layouts
//!
//! Uses a binary tree where each node is either a leaf (pane) or a split
//! containing two children.

use std::collections::HashMap;
use super::layout::PaneRect;
use super::Divider;

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
            PaneNode::Split { first, second, .. } => {
                first.contains(id) || second.contains(id)
            }
        }
    }

    /// Find and split a leaf node
    fn split_leaf(&mut self, target: PaneId, new_id: PaneId, direction: SplitDirection) -> bool {
        match self {
            PaneNode::Leaf(id) if *id == target => {
                // Replace this leaf with a split
                let old_node = Box::new(PaneNode::Leaf(*id));
                let new_node = Box::new(PaneNode::Leaf(new_id));
                *self = PaneNode::Split {
                    direction,
                    ratio: 0.5,
                    first: old_node,
                    second: new_node,
                };
                true
            }
            PaneNode::Leaf(_) => false,
            PaneNode::Split { first, second, .. } => {
                first.split_leaf(target, new_id, direction) ||
                second.split_leaf(target, new_id, direction)
            }
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
                    *first = Box::new(replacement);
                    return None;
                }
                if let Some(replacement) = second.remove_leaf(target) {
                    *second = Box::new(replacement);
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
                first.find_sibling(target).or_else(|| second.find_sibling(target))
            }
        }
    }

    /// Calculate layout for all panes in this subtree
    fn calculate_layout(&self, rect: PaneRect, result: &mut HashMap<PaneId, PaneRect>) {
        match self {
            PaneNode::Leaf(id) => {
                result.insert(*id, rect);
            }
            PaneNode::Split { direction, ratio, first, second } => {
                let (first_rect, second_rect) = match direction {
                    SplitDirection::Horizontal => {
                        // Split horizontally (top/bottom)
                        let divider_height: f32 = 2.0; // Divider thickness
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
                        let divider_width: f32 = 2.0; // Divider thickness
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

    /// Get dividers for rendering
    fn get_dividers(&self, rect: PaneRect, result: &mut Vec<Divider>) {
        if let PaneNode::Split { direction, ratio, first, second } = self {
            match direction {
                SplitDirection::Horizontal => {
                    let divider_y = rect.y + (rect.height - 2.0) * ratio;
                    result.push(Divider {
                        start: rect.x,
                        end: rect.x + rect.width,
                        position: divider_y,
                        vertical: false,
                    });

                    // Recurse into children
                    let first_height = (rect.height - 2.0) * ratio;
                    first.get_dividers(PaneRect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: first_height,
                    }, result);
                    second.get_dividers(PaneRect {
                        x: rect.x,
                        y: rect.y + first_height + 2.0,
                        width: rect.width,
                        height: rect.height - first_height - 2.0,
                    }, result);
                }
                SplitDirection::Vertical => {
                    let divider_x = rect.x + (rect.width - 2.0) * ratio;
                    result.push(Divider {
                        start: rect.y,
                        end: rect.y + rect.height,
                        position: divider_x,
                        vertical: true,
                    });

                    // Recurse into children
                    let first_width = (rect.width - 2.0) * ratio;
                    first.get_dividers(PaneRect {
                        x: rect.x,
                        y: rect.y,
                        width: first_width,
                        height: rect.height,
                    }, result);
                    second.get_dividers(PaneRect {
                        x: rect.x + first_width + 2.0,
                        y: rect.y,
                        width: rect.width - first_width - 2.0,
                        height: rect.height,
                    }, result);
                }
            }
        }
    }
}

/// The pane tree structure
#[derive(Debug)]
pub struct PaneTree {
    root: PaneNode,
}

impl PaneTree {
    /// Create a new tree with a single pane
    pub fn new(id: PaneId) -> Self {
        Self {
            root: PaneNode::Leaf(id),
        }
    }

    /// Split a pane
    pub fn split(&mut self, target: PaneId, new_id: PaneId, direction: SplitDirection) -> bool {
        self.root.split_leaf(target, new_id, direction)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_single_pane() {
        let tree = PaneTree::new(PaneId(0));
        let layout = tree.calculate_layout(PaneRect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 });

        assert_eq!(layout.len(), 1);
        let rect = layout.get(&PaneId(0)).unwrap();
        assert_eq!(rect.width, 800.0);
        assert_eq!(rect.height, 600.0);
    }

    #[test]
    fn test_vertical_split() {
        let mut tree = PaneTree::new(PaneId(0));
        tree.split(PaneId(0), PaneId(1), SplitDirection::Vertical);

        let layout = tree.calculate_layout(PaneRect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 });
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

        let layout = tree.calculate_layout(PaneRect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 });
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
}
