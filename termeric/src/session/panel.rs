//! Session management panel
//!
//! A slide-in panel for listing, loading, and deleting saved sessions.

use std::time::Instant;

/// Animation duration in seconds (matches settings panel)
const ANIMATION_DURATION: f32 = 0.2;

/// Information about a single session for display
#[derive(Debug, Clone)]
pub struct SessionEntry {
    /// Session directory name (used for load/delete operations)
    pub id: String,
    /// Display name shown to the user
    pub display_name: String,
    /// Number of tabs in the session
    pub tab_count: usize,
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
    /// Index of the highlighted/hovered session
    pub selected_index: usize,
}

/// Session management panel state
pub struct SessionPanel {
    /// Whether the panel is currently visible
    open: bool,
    /// When the current animation started
    animation_start: Instant,
    /// Whether we are animating open (true) or closed (false)
    animating_open: bool,
    /// The animation value at the time the direction changed
    animation_base: f32,
    /// Currently highlighted session index
    selected_index: usize,
    /// Cached session entries
    sessions: Vec<SessionEntry>,
}

impl SessionPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            animation_start: Instant::now(),
            animating_open: false,
            animation_base: 0.0,
            selected_index: 0,
            sessions: Vec::new(),
        }
    }

    /// Open the session management panel, refreshing the session list
    pub fn open(&mut self) {
        self.animation_base = self.raw_animation();
        self.open = true;
        self.animating_open = true;
        self.animation_start = Instant::now();
        self.selected_index = 0;
        self.refresh_sessions();
    }

    /// Close the panel
    pub fn close(&mut self) {
        self.animation_base = self.raw_animation();
        self.animating_open = false;
        self.animation_start = Instant::now();
    }

    /// Whether the panel is logically open (may still be animating closed)
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the panel is fully closed and invisible
    pub fn is_closed(&self) -> bool {
        !self.open && self.animation_progress() <= 0.0
    }

    /// Whether the panel needs redrawing (animation in progress)
    pub fn needs_redraw(&self) -> bool {
        let anim = self.animation_progress();
        if self.open {
            anim < 1.0
        } else {
            anim > 0.0
        }
    }

    /// Current animation progress (0.0 to 1.0)
    pub fn animation_progress(&self) -> f32 {
        let raw = self.raw_animation();
        // Ease-out cubic
        let t = raw;
        1.0 - (1.0 - t).powi(3)
    }

    fn raw_animation(&self) -> f32 {
        let elapsed = self.animation_start.elapsed().as_secs_f32();
        let linear = (elapsed / ANIMATION_DURATION).min(1.0);

        if self.animating_open {
            // Opening: animate from base to 1.0
            self.animation_base + (1.0 - self.animation_base) * linear
        } else {
            // Closing: animate from base to 0.0
            self.animation_base * (1.0 - linear)
        }
    }

    /// Mark the panel as fully closed once animation completes
    pub fn update(&mut self) {
        if !self.animating_open && self.raw_animation() <= 0.0 {
            self.open = false;
        }
    }

    /// Refresh the session list from disk
    pub fn refresh_sessions(&mut self) {
        let names = super::list_sessions();
        self.sessions = names.iter().map(|name| {
            // Try to load the session title from the session file
            let display_name = super::session_file_path(name)
                .and_then(|path| std::fs::read_to_string(&path).ok())
                .and_then(|content| {
                    // Parse session title from TOML
                    content.lines()
                        .find(|line| line.starts_with("title"))
                        .and_then(|line| {
                            line.split('=').nth(1)
                                .map(|v| v.trim().trim_matches('"').to_string())
                        })
                })
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| name.clone());

            // Try to read last modified time
            let last_modified = match super::session_file_path(name) {
                Some(path) if path.exists() => {
                    std::fs::metadata(&path)
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
                        .unwrap_or_else(|| "unknown".to_string())
                }
                _ => "unknown".to_string(),
            };

            SessionEntry {
                id: name.clone(),
                display_name,
                tab_count: 1,
                last_modified,
            }
        }).collect();
    }

    /// Build info for the renderer
    pub fn build_info(&self) -> SessionPanelInfo {
        SessionPanelInfo {
            animation: self.animation_progress(),
            sessions: self.sessions.clone(),
            selected_index: self.selected_index,
        }
    }

    /// Get the sessions list
    pub fn sessions(&self) -> &[SessionEntry] {
        &self.sessions
    }
}
