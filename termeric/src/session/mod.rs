//! Session save/restore functionality
//!
//! Each session represents a single terminal tab's state:
//! - Working directory
//! - Scrollback history (as a text file)
//! - Title and metadata
//!
//! Sessions are identified by a timestamp-based ID (e.g., "2026-02-24_21h59m43").
//! Each tab independently tracks its own session identity.

pub mod save;
pub mod restore;
pub mod panel;

pub use save::save_session;
pub use restore::restore_session;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A saved terminal session (single tab)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Version for compatibility checking
    pub version: u32,
    /// Session ID (timestamp-based, e.g., "2026-02-24_21h59m43")
    pub id: String,
    /// Tab title
    pub title: String,
    /// Working directory when saved
    pub working_directory: Option<String>,
    /// Relative path to scrollback text file (within session directory)
    #[serde(default)]
    pub scrollback_file: Option<String>,
    /// When this session was first created (ISO 8601)
    pub created_at: String,
    /// When this session was last updated (ISO 8601)
    pub updated_at: String,
}

impl Session {
    /// Current session format version
    pub const CURRENT_VERSION: u32 = 3;

    /// Create a new session with a generated ID
    pub fn new(title: String, working_directory: Option<String>) -> Self {
        let now = now_iso8601();
        let id = generate_session_id();
        Self {
            version: Self::CURRENT_VERSION,
            id,
            title,
            working_directory,
            scrollback_file: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Create a session with a specific ID (for updating existing sessions)
    pub fn with_id(id: String, title: String, working_directory: Option<String>) -> Self {
        let now = now_iso8601();
        Self {
            version: Self::CURRENT_VERSION,
            id,
            title,
            working_directory,
            scrollback_file: None,
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Check if the session is compatible with the current version
    pub fn is_compatible(&self) -> bool {
        self.version <= Self::CURRENT_VERSION
    }
}

/// Generate a timestamp-based session ID
///
/// Format: "2026-02-24_21h59m43"
pub fn generate_session_id() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();

    // Convert to broken-down time manually (UTC)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to Y-M-D (simplified leap year calculation)
    let mut year = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let month_days = if is_leap_year(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 0;
    for (i, &d) in month_days.iter().enumerate() {
        if remaining_days < d {
            month = i + 1;
            break;
        }
        remaining_days -= d;
    }
    let day = remaining_days + 1;

    format!(
        "{:04}-{:02}-{:02}_{:02}h{:02}m{:02}",
        year, month, day, hours, minutes, seconds
    )
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Get current time as ISO 8601 string
fn now_iso8601() -> String {
    use std::time::SystemTime;
    let duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // Reuse the ID generator logic for the timestamp
    let id = generate_session_id();
    // Convert to ISO format: "2026-02-24T21:59:43Z"
    let parts: Vec<&str> = id.splitn(2, '_').collect();
    if parts.len() == 2 {
        let time_part = parts[1]
            .replace('h', ":")
            .replace('m', ":");
        format!("{}T{}Z", parts[0], time_part)
    } else {
        format!("{}Z", duration.as_secs())
    }
}

/// Get the base sessions directory
///
/// Returns `~/.local/share/termeric/sessions/`
pub fn sessions_base_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "termeric", "termeric")
        .map(|dirs| dirs.data_dir().join("sessions"))
}

/// Sanitize a session name to prevent path traversal attacks.
///
/// Returns `None` if the name is invalid (contains `/`, `\`, `..`,
/// is empty, or starts with `.`).
pub fn sanitize_session_name(name: &str) -> Option<&str> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.starts_with('.')
    {
        log::warn!("Rejected invalid session name: {:?}", name);
        None
    } else {
        Some(name)
    }
}

/// Get the directory for a specific session
///
/// Named session → `sessions/<name>/`
///
/// Returns `None` if the name fails sanitization.
pub fn session_dir(name: &str) -> Option<PathBuf> {
    sanitize_session_name(name)?;
    sessions_base_dir().map(|base| base.join(name))
}

/// Get the session.toml path for a named session
pub fn session_file_path(name: &str) -> Option<PathBuf> {
    session_dir(name).map(|dir| dir.join("session.toml"))
}

/// List all available saved sessions
///
/// Returns a list of session names sorted newest-first (reverse alphabetical
/// on timestamp-based IDs).
pub fn list_sessions() -> Vec<String> {
    let Some(base) = sessions_base_dir() else {
        return Vec::new();
    };

    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };

    let mut sessions: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| e.path().join("session.toml").exists())
        .filter_map(|e| e.file_name().into_string().ok())
        // Skip legacy "default" session
        .filter(|name| name != "default")
        .collect();

    // Sort newest first (reverse alphabetical works for our timestamp format)
    sessions.sort_by(|a, b| b.cmp(a));

    sessions
}

/// List all saved sessions as (id, display_title) pairs
///
/// For each session, reads the title from session.toml. Falls back to the
/// session ID if the title can't be read.
pub fn list_sessions_with_titles() -> Vec<(String, String)> {
    list_sessions()
        .into_iter()
        .map(|id| {
            let title = session_file_path(&id)
                .and_then(|path| std::fs::read_to_string(&path).ok())
                .and_then(|content| {
                    content.lines()
                        .find(|line| line.starts_with("title"))
                        .and_then(|line| {
                            line.split('=').nth(1)
                                .map(|v| v.trim().trim_matches('"').to_string())
                        })
                })
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| id.clone());
            (id, title)
        })
        .collect()
}

/// Delete a saved session by name
///
/// Returns an error if the name fails sanitization.
pub fn delete_session(name: &str) -> anyhow::Result<()> {
    // Sanitize before doing anything with the filesystem
    let _ = sanitize_session_name(name)
        .ok_or_else(|| anyhow::anyhow!("Invalid session name: {:?}", name))?;

    let dir = session_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;

    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
        log::info!("Deleted session '{}' at {:?}", name, dir);
    }

    Ok(())
}

/// Maximum scrollback file size (5 MB)
pub const MAX_SCROLLBACK_FILE_SIZE: usize = 5 * 1024 * 1024;
