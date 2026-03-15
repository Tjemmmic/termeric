//! Pane management for split terminal views
//!
//! This module provides support for splitting a tab into multiple terminal panes,
//! arranged in a binary tree structure for flexible horizontal and vertical splits.

mod tree;
mod layout;

pub use tree::{PaneTree, PaneId, SplitDirection};
pub use layout::PaneRect;

use std::collections::HashMap;
use std::io;
use std::sync::mpsc::Receiver;
use termeric_core::{Pty, Terminal, TerminalEvent};
use termeric_core::pty::{PtyEvent, PtyHandle, spawn_reader_thread};
use crate::config::ShellConfig;
use crate::terminal::container::{TerminalContainer, SplitContainer};

/// A single pane containing a terminal
pub struct Pane {
    /// Unique pane ID
    pub id: PaneId,
    /// Terminal state
    pub terminal: Terminal,
    /// PTY handle for writing
    pub pty: PtyHandle,
    /// PTY output receiver
    pub pty_rx: Receiver<PtyEvent>,
    /// Whether the shell process has exited
    pub closed: bool,
    /// Current pane dimensions (cols, rows)
    pub size: (usize, usize),
}

impl Pane {
    /// Create a new pane with a shell
    pub fn new(id: PaneId, cols: usize, rows: usize, shell: &ShellConfig, scrollback_lines: usize) -> anyhow::Result<Self> {
        let mut terminal = Terminal::new(cols, rows);
        terminal.grid_mut().set_scrollback_limit(scrollback_lines);
        let pty = Pty::new_with_command(
            cols as u16,
            rows as u16,
            shell.program.as_deref(),
            &shell.args,
        )?;
        let (handle, reader) = pty.take_reader();
        let rx = spawn_reader_thread(reader);

        Ok(Self {
            id,
            terminal,
            pty: handle,
            pty_rx: rx,
            closed: false,
            size: (cols, rows),
        })
    }

    /// Process any pending PTY events
    pub fn process_events(&mut self) -> bool {
        let mut had_output = false;

        while let Ok(event) = self.pty_rx.try_recv() {
            match event {
                PtyEvent::Output(data) => {
                    self.terminal.process(&data);
                    had_output = true;
                }
                PtyEvent::Closed => {
                    log::info!("Pane {:?} shell exited", self.id);
                    self.closed = true;
                }
                PtyEvent::Error(e) => {
                    log::error!("Pane {:?} PTY error: {}", self.id, e);
                    self.closed = true;
                }
            }
        }

        had_output
    }

    /// Write data to the PTY
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        self.pty.write_all(data)
    }

    /// Resize the terminal and PTY
    pub fn resize(&mut self, cols: usize, rows: usize) -> anyhow::Result<()> {
        if (cols, rows) != self.size && cols > 0 && rows > 0 {
            self.terminal.resize(cols, rows);
            self.pty.resize(cols as u16, rows as u16)?;
            self.size = (cols, rows);
        }
        Ok(())
    }
}

/// Manager for panes within a single tab
pub struct PaneManager {
    /// The pane tree structure
    tree: PaneTree,
    /// Map of pane IDs to panes
    panes: HashMap<PaneId, Pane>,
    /// Currently focused pane
    focused: PaneId,
    /// Shell configuration for new panes
    shell: ShellConfig,
    /// Scrollback limit for new panes
    scrollback_lines: usize,
    /// Next pane ID
    next_id: u32,
    /// Total available width in pixels (for layout calculations)
    total_width: f32,
    /// Total available height in pixels
    total_height: f32,
    /// Cell width in pixels
    cell_width: f32,
    /// Cell height in pixels
    cell_height: f32,
}

impl PaneManager {
    /// Create a new pane manager with a single pane
    pub fn new(
        cols: usize,
        rows: usize,
        shell: ShellConfig,
        scrollback_lines: usize,
        total_width: f32,
        total_height: f32,
        cell_width: f32,
        cell_height: f32,
    ) -> anyhow::Result<Self> {
        let id = PaneId(0);
        let pane = Pane::new(id, cols, rows, &shell, scrollback_lines)?;

        let mut panes = HashMap::new();
        panes.insert(id, pane);

        Ok(Self {
            tree: PaneTree::new(id),
            panes,
            focused: id,
            shell,
            scrollback_lines,
            next_id: 1,
            total_width,
            total_height,
            cell_width,
            cell_height,
        })
    }

    /// Get the focused pane
    pub fn focused_pane(&self) -> Option<&Pane> {
        self.panes.get(&self.focused)
    }

    /// Get the focused pane mutably
    pub fn focused_pane_mut(&mut self) -> Option<&mut Pane> {
        self.panes.get_mut(&self.focused)
    }

    /// Get the focused pane ID
    pub fn focused_id(&self) -> PaneId {
        self.focused
    }

    /// Set focus to a specific pane
    pub fn focus(&mut self, id: PaneId) {
        if self.panes.contains_key(&id) {
            self.focused = id;
        }
    }

    /// Get a pane by ID
    pub fn get_pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    /// Get a pane by ID mutably
    pub fn get_pane_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        self.panes.get_mut(&id)
    }

    /// Split the focused pane
    pub fn split(&mut self, direction: SplitDirection) -> anyhow::Result<PaneId> {
        let new_id = PaneId(self.next_id);
        self.next_id += 1;

        // Calculate size for the new pane (roughly half of current)
        let layout = self.calculate_layout();
        let parent_rect = layout.get(&self.focused).cloned().unwrap_or(PaneRect {
            x: 0.0,
            y: 0.0,
            width: self.total_width,
            height: self.total_height,
        });

        let (cols, rows) = match direction {
            SplitDirection::Horizontal => {
                // Split horizontally (new pane below)
                let new_height = parent_rect.height / 2.0;
                let cols = (parent_rect.width / self.cell_width).floor() as usize;
                let rows = (new_height / self.cell_height).floor() as usize;
                (cols.max(1), rows.max(1))
            }
            SplitDirection::Vertical => {
                // Split vertically (new pane to the right)
                let new_width = parent_rect.width / 2.0;
                let cols = (new_width / self.cell_width).floor() as usize;
                let rows = (parent_rect.height / self.cell_height).floor() as usize;
                (cols.max(1), rows.max(1))
            }
        };

        let pane = Pane::new(new_id, cols, rows, &self.shell, self.scrollback_lines)?;
        self.panes.insert(new_id, pane);

        // Split the tree
        self.tree.split(self.focused, new_id, direction);

        // Focus the new pane
        self.focused = new_id;

        // Recalculate all pane sizes
        self.recalculate_sizes();

        log::info!("Split pane {:?} {:?}, created pane {:?}", self.focused, direction, new_id);
        Ok(new_id)
    }

    /// Close the focused pane
    pub fn close_focused(&mut self) -> bool {
        if self.panes.len() <= 1 {
            return false; // Can't close the last pane
        }

        let to_close = self.focused;

        // Find a sibling to focus
        if let Some(sibling) = self.tree.find_sibling(to_close) {
            self.focused = sibling;
        } else {
            // Fallback to any remaining pane
            if let Some(&id) = self.panes.keys().find(|&&id| id != to_close) {
                self.focused = id;
            } else {
                return false;
            }
        }

        // Remove from tree and panes
        self.tree.remove(to_close);
        self.panes.remove(&to_close);

        // Recalculate sizes
        self.recalculate_sizes();

        log::info!("Closed pane {:?}, focused {:?}", to_close, self.focused);
        true
    }

    /// Navigate to an adjacent pane
    pub fn navigate(&mut self, direction: SplitDirection) {
        // Find pane in the given direction
        let layout = self.calculate_layout();
        if let Some(current_rect) = layout.get(&self.focused) {
            let target = match direction {
                SplitDirection::Vertical => {
                    // Move right
                    layout.iter()
                        .filter(|(id, rect)| {
                            **id != self.focused &&
                            rect.x > current_rect.x + current_rect.width * 0.5 &&
                            rects_overlap_vertically(current_rect, rect)
                        })
                        .min_by(|(_, a), (_, b)| a.x.partial_cmp(&b.x).unwrap())
                        .map(|(id, _)| *id)
                }
                SplitDirection::Horizontal => {
                    // Move down
                    layout.iter()
                        .filter(|(id, rect)| {
                            **id != self.focused &&
                            rect.y > current_rect.y + current_rect.height * 0.5 &&
                            rects_overlap_horizontally(current_rect, rect)
                        })
                        .min_by(|(_, a), (_, b)| a.y.partial_cmp(&b.y).unwrap())
                        .map(|(id, _)| *id)
                }
            };

            if let Some(id) = target {
                self.focused = id;
            }
        }
    }

    /// Process events for all panes
    pub fn process_all_events(&mut self) -> bool {
        let mut any_output = false;
        for pane in self.panes.values_mut() {
            any_output |= pane.process_events();
        }
        any_output
    }

    /// Resize the total available space
    pub fn resize(&mut self, total_width: f32, total_height: f32, cell_width: f32, cell_height: f32) {
        self.total_width = total_width;
        self.total_height = total_height;
        self.cell_width = cell_width;
        self.cell_height = cell_height;
        self.recalculate_sizes();
    }

    /// Calculate the layout for all panes
    pub fn calculate_layout(&self) -> HashMap<PaneId, PaneRect> {
        let root_rect = PaneRect {
            x: 0.0,
            y: 0.0,
            width: self.total_width,
            height: self.total_height,
        };
        self.tree.calculate_layout(root_rect)
    }

    /// Recalculate and apply sizes to all panes
    fn recalculate_sizes(&mut self) {
        let layout = self.calculate_layout();

        for (id, rect) in layout {
            if let Some(pane) = self.panes.get_mut(&id) {
                let cols = (rect.width / self.cell_width).floor() as usize;
                let rows = (rect.height / self.cell_height).floor() as usize;
                if let Err(e) = pane.resize(cols.max(1), rows.max(1)) {
                    log::error!("Failed to resize pane {:?}: {}", id, e);
                }
            }
        }
    }

    /// Get dividers for rendering
    pub fn get_dividers(&self) -> Vec<Divider> {
        self.tree.get_dividers(PaneRect {
            x: 0.0,
            y: 0.0,
            width: self.total_width,
            height: self.total_height,
        })
    }

    /// Resize a split divider at the given position (in content-local coordinates).
    ///
    /// `threshold` controls how close the mouse must be to match a divider.
    /// Use a small value for click detection, or `f32::MAX` during active drag.
    /// Returns true if a divider was hit and resized.
    pub fn resize_divider_at(&mut self, pos_x: f32, pos_y: f32, threshold: f32) -> bool {
        let rect = PaneRect {
            x: 0.0, y: 0.0,
            width: self.total_width, height: self.total_height,
        };
        if self.tree.resize_at(rect, pos_x, pos_y, threshold) {
            // Recalculate pane sizes after ratio change
            self.recalculate_sizes();
            true
        } else {
            false
        }
    }

    /// Hit-test dividers. Returns Some(is_vertical) if near a divider.
    pub fn hit_test_divider(&self, pos_x: f32, pos_y: f32) -> Option<bool> {
        let rect = PaneRect {
            x: 0.0, y: 0.0,
            width: self.total_width, height: self.total_height,
        };
        self.tree.hit_test_divider(rect, pos_x, pos_y)
    }

    /// Get number of panes
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Take the last remaining pane out of the manager.
    ///
    /// Returns `Some(pane)` if exactly one pane remains, consuming it from the
    /// internal map. Used when dissolving pane mode back to single-terminal tab.
    pub fn take_remaining_pane(&mut self) -> Option<Pane> {
        if self.panes.len() != 1 {
            return None;
        }
        let id = *self.panes.keys().next()?;
        self.panes.remove(&id)
    }

    /// Iterate over all panes with their layouts
    pub fn iter_with_layout(&self) -> impl Iterator<Item = (&Pane, PaneRect)> {
        let layout = self.calculate_layout();
        self.panes.values().filter_map(move |pane| {
            layout.get(&pane.id).map(|&rect| (pane, rect))
        })
    }

    /// Remove closed panes
    pub fn cleanup_closed(&mut self) -> bool {
        let closed: Vec<PaneId> = self.panes
            .iter()
            .filter(|(_, pane)| pane.closed)
            .map(|(&id, _)| id)
            .collect();

        for id in closed {
            if self.panes.len() > 1 {
                if id == self.focused {
                    // Focus another pane first
                    if let Some(&new_focus) = self.panes.keys().find(|&&pid| pid != id) {
                        self.focused = new_focus;
                    }
                }
                self.tree.remove(id);
                self.panes.remove(&id);
            }
        }

        !self.panes.is_empty()
    }

    /// Check if all panes are closed
    pub fn all_closed(&self) -> bool {
        self.panes.is_empty() || self.panes.values().all(|p| p.closed)
    }
}

/// Implement TerminalContainer for PaneManager
impl TerminalContainer for PaneManager {
    fn focused_terminal(&self) -> Option<&Terminal> {
        self.focused_pane().map(|p| &p.terminal)
    }

    fn focused_terminal_mut(&mut self) -> Option<&mut Terminal> {
        self.focused_pane_mut().map(|p| &mut p.terminal)
    }

    fn write_input(&mut self, data: &[u8]) -> io::Result<()> {
        if let Some(pane) = self.focused_pane_mut() {
            pane.write(data)
        } else {
            Ok(())
        }
    }

    fn process_events(&mut self) -> (bool, Vec<TerminalEvent>) {
        let mut any_output = false;
        let mut all_events = Vec::new();

        for pane in self.panes.values_mut() {
            let had_output = pane.process_events();
            any_output = any_output || had_output;
            all_events.extend(pane.terminal.drain_events());
        }

        (any_output, all_events)
    }

    fn resize(&mut self, _cols: usize, _rows: usize) -> anyhow::Result<()> {
        // PaneManager handles resize differently - via resize() method with pixel dimensions
        // This is called with cols/rows but PaneManager needs pixel dimensions
        // Recalculate all pane sizes based on current total dimensions
        self.recalculate_sizes();
        Ok(())
    }

    fn is_closed(&self) -> bool {
        self.all_closed()
    }
}

/// Implement SplitContainer for PaneManager
impl SplitContainer for PaneManager {
    fn split(&mut self, direction: SplitDirection) -> anyhow::Result<PaneId> {
        PaneManager::split(self, direction)
    }

    fn close_focused_pane(&mut self) -> bool {
        self.close_focused()
    }

    fn navigate(&mut self, direction: SplitDirection) {
        PaneManager::navigate(self, direction)
    }

    fn pane_count(&self) -> usize {
        PaneManager::pane_count(self)
    }

    fn pane_layout(&self) -> HashMap<PaneId, PaneRect> {
        self.calculate_layout()
    }

    fn dividers(&self) -> Vec<Divider> {
        self.get_dividers()
    }

    fn focused_pane_id(&self) -> PaneId {
        self.focused_id()
    }

    fn iter_panes(&self) -> Vec<(&Terminal, PaneRect, bool)> {
        let layout = self.calculate_layout();
        self.panes.values()
            .filter_map(|pane| {
                layout.get(&pane.id).map(|&rect| {
                    (&pane.terminal, rect, pane.id == self.focused)
                })
            })
            .collect()
    }
}

/// A divider between panes
#[derive(Debug, Clone, Copy)]
pub struct Divider {
    /// Start position (x for vertical, y for horizontal)
    pub start: f32,
    /// End position
    pub end: f32,
    /// Position perpendicular to orientation
    pub position: f32,
    /// Whether this is a vertical divider
    pub vertical: bool,
}

/// Check if two rectangles overlap vertically
fn rects_overlap_vertically(a: &PaneRect, b: &PaneRect) -> bool {
    a.y < b.y + b.height && a.y + a.height > b.y
}

/// Check if two rectangles overlap horizontally
fn rects_overlap_horizontally(a: &PaneRect, b: &PaneRect) -> bool {
    a.x < b.x + b.width && a.x + a.width > b.x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pane_id() {
        let id1 = PaneId(0);
        let id2 = PaneId(1);
        assert_ne!(id1, id2);
        assert_eq!(id1, PaneId(0));
    }
}
