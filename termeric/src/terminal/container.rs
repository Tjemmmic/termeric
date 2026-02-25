//! Terminal container traits for abstracted terminal management
//!
//! This module provides traits that abstract over single terminal and split pane
//! containers, allowing the application to work uniformly with both.

use std::collections::HashMap;
use std::io;
use termeric_core::{Terminal, TerminalEvent};

use crate::panes::{PaneId, PaneRect, SplitDirection, Divider};

/// A container that holds one or more terminals
///
/// This trait abstracts over single-terminal tabs and multi-pane tabs,
/// providing a uniform interface for the application.
pub trait TerminalContainer {
    /// Get the focused terminal (read-only)
    fn focused_terminal(&self) -> Option<&Terminal>;

    /// Get the focused terminal (mutable)
    fn focused_terminal_mut(&mut self) -> Option<&mut Terminal>;

    /// Write input data to the focused terminal's PTY
    fn write_input(&mut self, data: &[u8]) -> io::Result<()>;

    /// Process pending events from all terminals
    /// Returns (had_output, terminal_events)
    fn process_events(&mut self) -> (bool, Vec<TerminalEvent>);

    /// Resize all terminals to fit the given dimensions
    fn resize(&mut self, cols: usize, rows: usize) -> anyhow::Result<()>;

    /// Check if the container is closed (all terminals exited)
    fn is_closed(&self) -> bool;
}

/// Extended container that supports split panes
///
/// Containers implementing this trait support splitting into multiple panes.
pub trait SplitContainer: TerminalContainer {
    /// Split the focused pane in the given direction
    fn split(&mut self, direction: SplitDirection) -> anyhow::Result<PaneId>;

    /// Close the focused pane
    /// Returns true if a pane was closed, false if this is the last pane
    fn close_focused_pane(&mut self) -> bool;

    /// Navigate focus to an adjacent pane
    fn navigate(&mut self, direction: SplitDirection);

    /// Get the number of panes
    fn pane_count(&self) -> usize;

    /// Get the layout of all panes
    fn pane_layout(&self) -> HashMap<PaneId, PaneRect>;

    /// Get dividers between panes for rendering
    fn dividers(&self) -> Vec<Divider>;

    /// Get the focused pane ID
    fn focused_pane_id(&self) -> PaneId;

    /// Iterate over all panes with their terminals and layouts
    fn iter_panes(&self) -> Vec<(&Terminal, PaneRect, bool)>;
}

/// Render information for a pane
#[derive(Debug, Clone)]
pub struct PaneRenderInfo {
    /// The pane's layout rectangle
    pub rect: PaneRect,
    /// Whether this pane is focused
    pub is_focused: bool,
    /// Pane ID
    pub id: PaneId,
}
