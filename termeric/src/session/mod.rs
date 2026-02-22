//! Session save/restore functionality
//!
//! Allows saving and restoring terminal sessions including:
//! - Tab state (working directories, titles)
//! - Pane layout within tabs
//! - Window position and size

mod save;
mod restore;

pub use save::save_session;
pub use restore::restore_session;

use serde::{Deserialize, Serialize};

/// A saved terminal session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Version for compatibility checking
    pub version: u32,
    /// Saved tabs
    pub tabs: Vec<SavedTab>,
    /// Index of the active tab
    pub active_tab: usize,
    /// Window dimensions (width, height)
    pub window_size: Option<(u32, u32)>,
    /// Window position (x, y), if known
    pub window_position: Option<(i32, i32)>,
}

/// A saved tab
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTab {
    /// Tab title
    pub title: String,
    /// Working directory when saved
    pub working_directory: Option<String>,
    /// Pane layout (for split panes)
    pub panes: Vec<SavedPane>,
    /// Index of the focused pane within this tab
    pub focused_pane: usize,
}

/// A saved pane within a tab
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPane {
    /// Working directory
    pub working_directory: Option<String>,
    /// Pane position in layout
    pub position: PanePosition,
}

/// Position of a pane in the layout
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanePosition {
    /// Relative x position (0.0 - 1.0)
    pub x: f32,
    /// Relative y position (0.0 - 1.0)
    pub y: f32,
    /// Relative width (0.0 - 1.0)
    pub width: f32,
    /// Relative height (0.0 - 1.0)
    pub height: f32,
}

impl Session {
    /// Current session format version
    pub const CURRENT_VERSION: u32 = 1;

    /// Create a new empty session
    pub fn new() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            tabs: Vec::new(),
            active_tab: 0,
            window_size: None,
            window_position: None,
        }
    }

    /// Check if the session is compatible with the current version
    pub fn is_compatible(&self) -> bool {
        self.version <= Self::CURRENT_VERSION
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl SavedTab {
    /// Create a new saved tab with just a working directory
    pub fn new(title: String, working_directory: Option<String>) -> Self {
        Self {
            title,
            working_directory: working_directory.clone(),
            panes: vec![SavedPane {
                working_directory,
                position: PanePosition {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
            }],
            focused_pane: 0,
        }
    }
}

/// Get the session file path
pub fn session_file_path() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("com", "termeric", "termeric")
        .map(|dirs| dirs.data_dir().join("session.toml"))
}
