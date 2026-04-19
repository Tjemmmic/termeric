//! Session management panel
//!
//! A slide-in panel for listing, loading, and deleting saved sessions.

use crate::ui::animation::PanelAnimation;

/// Information about a single session for display
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Session directory name (used for load/delete operations)
    pub id: String,
    /// Display name shown to the user
    pub display_name: String,
    /// Short metadata string shown below the name (last command, cwd basename, etc.)
    pub subtitle: String,
    /// Last modified time as a human-readable string
    pub last_modified: String,
}

/// Click result from the session panel
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionPanelAction {
    /// Load a session by its id
    Load(String),
    /// Delete a session by its id
    Delete(String),
    /// Close the panel
    Close,
    /// No action
    None,
}

/// Info passed to the renderer
#[derive(Debug, Clone)]
pub struct SessionPanelInfo {
    /// Animation progress (0.0 = hidden, 1.0 = fully visible)
    pub animation: f32,
    /// List of sessions to display
    pub sessions: Vec<SessionEntry>,
    /// Mouse-hovered session index (None if not hovering any session)
    pub hovered_index: Option<usize>,
}

/// Session management panel state
pub struct SessionPanel {
    /// Animation state
    animation: PanelAnimation,
    /// Currently highlighted session index
    selected_index: usize,
    /// Cached session entries
    sessions: Vec<SessionEntry>,
    /// Mouse-hovered session index
    hovered_index: Option<usize>,
}

impl SessionPanel {
    pub fn new() -> Self {
        Self {
            animation: PanelAnimation::new(),
            selected_index: 0,
            sessions: Vec::new(),
            hovered_index: None,
        }
    }

    /// Open the session management panel, refreshing the session list
    pub fn open(&mut self) {
        self.animation.open();
        self.selected_index = 0;
        self.refresh_sessions();
    }

    /// Close the panel
    pub fn close(&mut self) {
        self.animation.close();
    }

    /// Whether the panel is logically open (may still be animating closed)
    pub fn is_open(&self) -> bool {
        self.animation.is_open()
    }

    /// Whether the panel is fully closed and invisible
    pub fn is_closed(&self) -> bool {
        self.animation.is_closed()
    }

    /// Whether the panel needs redrawing (animation in progress)
    pub fn needs_redraw(&self) -> bool {
        self.animation.needs_redraw()
    }

    /// Current animation progress (0.0 to 1.0)
    pub fn animation_progress(&self) -> f32 {
        self.animation.progress()
    }

    /// Mark the panel as fully closed once animation completes
    pub fn update(&mut self) {
        self.animation.update();
    }

    /// Refresh the session list from disk
    pub fn refresh_sessions(&mut self) {
        let names = super::list_sessions();
        self.sessions = names
            .iter()
            .map(|name| {
                // Parse the session TOML once so we can pull title, last_command,
                // and working_directory out of the same read.
                let session_text = super::session_file_path(name)
                    .and_then(|path| std::fs::read_to_string(&path).ok());
                let field = |key: &str| -> Option<String> {
                    session_text.as_ref().and_then(|s| {
                        s.lines()
                            .find(|line| line.starts_with(key))
                            .and_then(|line| {
                                line.split('=')
                                    .nth(1)
                                    .map(|v| v.trim().trim_matches('"').to_string())
                            })
                            .filter(|v| !v.is_empty())
                    })
                };

                let display_name = field("title").unwrap_or_else(|| name.clone());
                // Subtitle prefers last_command, falls back to the cwd basename.
                let subtitle = field("last_command")
                    .or_else(|| {
                        field("working_directory").map(|cwd| {
                            std::path::Path::new(&cwd)
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or(cwd)
                        })
                    })
                    .unwrap_or_default();

                // Try to read last modified time
                let last_modified = match super::session_file_path(name) {
                    Some(path) if path.exists() => std::fs::metadata(&path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| {
                            let elapsed = t.elapsed().unwrap_or_default();
                            let secs = elapsed.as_secs();
                            if secs < 60 {
                                "just now".to_string()
                            } else if secs < 3600 {
                                format!("{}m ago", secs / 60)
                            } else if secs < 86400 {
                                format!("{}h ago", secs / 3600)
                            } else {
                                format!("{}d ago", secs / 86400)
                            }
                        })
                        .unwrap_or_else(|| "unknown".to_string()),
                    _ => "unknown".to_string(),
                };

                SessionEntry {
                    id: name.clone(),
                    display_name,
                    subtitle,
                    last_modified,
                }
            })
            .collect();
    }

    /// Build info for the renderer
    pub fn build_info(&self) -> SessionPanelInfo {
        SessionPanelInfo {
            animation: self.animation_progress(),
            sessions: self.sessions.clone(),
            hovered_index: self.hovered_index,
        }
    }

    /// Update hover state from mouse position
    pub fn set_hover(&mut self, index: Option<usize>) {
        self.hovered_index = index;
    }

    /// Get the sessions list
    pub fn sessions(&self) -> &[SessionEntry] {
        &self.sessions
    }
}
