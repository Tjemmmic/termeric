//! Session save/restore functionality
//!
//! Each session represents a single terminal tab's state:
//! - Working directory
//! - Bounded text scrollback (embedded in the version 5 snapshot)
//! - Title and metadata
//!
//! Sessions are identified by a timestamp-based ID (e.g., "2026-02-24_21h59m43").
//! Each tab independently tracks its own session identity.

pub mod export;
pub(crate) mod export_job;
mod file_io;
pub mod panel;
pub mod restore;
pub mod save;
mod snapshot;
pub(crate) mod terminal_state;
pub mod transcript;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
thread_local! {
    static TEST_SESSIONS_BASE_DIR: std::cell::RefCell<Option<PathBuf>> = const {
        std::cell::RefCell::new(None)
    };
}

static SESSION_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The live terminal scope represented by a saved session snapshot.
///
/// Older session files predate split panes and deserialize to
/// [`SessionCaptureScope::SingleTerminal`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCaptureScope {
    #[default]
    SingleTerminal,
    FocusedPane,
}

/// A saved terminal session (single tab)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Last foreground command that was running (e.g., "cargo", "vim")
    #[serde(default)]
    pub last_command: Option<String>,
    /// Whether this snapshot came from a whole single-terminal tab or only the
    /// focused pane of a split tab.
    #[serde(default)]
    pub capture_scope: SessionCaptureScope,
    /// When this session was first created (ISO 8601)
    pub created_at: String,
    /// When this session was last updated (ISO 8601)
    pub updated_at: String,
    /// Version 5 embeds text in the same atomically published file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_content: Option<SnapshotContent>,
    /// Hash of the selected metadata header; never written to disk.
    #[serde(skip)]
    pub(crate) snapshot_identity: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotContent {
    pub bytes: usize,
    pub sha256: String,
}

impl Session {
    /// Current session format version
    pub const CURRENT_VERSION: u32 = 5;

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
            last_command: None,
            capture_scope: SessionCaptureScope::SingleTerminal,
            created_at: now.clone(),
            updated_at: now,
            snapshot_content: None,
            snapshot_identity: None,
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
            last_command: None,
            capture_scope: SessionCaptureScope::SingleTerminal,
            created_at: now.clone(),
            updated_at: now,
            snapshot_content: None,
            snapshot_identity: None,
        }
    }

    /// Check if the session is compatible with the current version
    pub fn is_compatible(&self) -> bool {
        self.version <= Self::CURRENT_VERSION
    }
}

/// Generate a sortable, collision-resistant session ID. Nanoseconds, process
/// ID, and a process-local atomic sequence avoid same-tick collisions.
pub fn generate_session_id() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sequence = SESSION_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "{}_{}_{:010}_{:06}",
        format_unix_timestamp(duration.as_secs(), None).replace([':', 'T', 'Z'], ""),
        duration.subsec_nanos(),
        std::process::id(),
        sequence % 1_000_000,
    )
}

/// Get current time as ISO 8601 string
fn now_iso8601() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format_unix_timestamp(duration.as_secs(), Some(duration.subsec_millis()))
}

/// Format a non-negative Unix timestamp as UTC RFC 3339.
pub fn format_unix_timestamp(seconds: u64, millis: Option<u32>) -> String {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    match millis {
        Some(millis) => {
            format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
        }
        None => format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"),
    }
}

// Howard Hinnant's civil-from-days algorithm, with day zero at 1970-01-01.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

/// Get the base sessions directory
///
/// Returns `~/.local/share/termeric/sessions/`
pub fn sessions_base_dir() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_SESSIONS_BASE_DIR.with(|base| base.borrow().clone()) {
        return Some(path);
    }
    directories::ProjectDirs::from("com", "termeric", "termeric")
        .map(|dirs| dirs.data_dir().join("sessions"))
}

#[cfg(test)]
pub(crate) fn set_test_sessions_base_dir(path: Option<PathBuf>) {
    TEST_SESSIONS_BASE_DIR.with(|base| *base.borrow_mut() = path);
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

/// Delete a saved session by name
///
/// Returns an error if the name fails sanitization.
pub fn delete_session(name: &str) -> anyhow::Result<()> {
    // Sanitize before doing anything with the filesystem
    let _ = sanitize_session_name(name)
        .ok_or_else(|| anyhow::anyhow!("Invalid session name: {:?}", name))?;

    let dir = session_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;

    if !dir.exists() {
        anyhow::bail!("Session {name:?} was not found");
    }

    std::fs::remove_dir_all(&dir)?;
    log::info!("Deleted session '{}' at {:?}", name, dir);

    Ok(())
}

/// Maximum scrollback file size (5 MB)
pub const MAX_SCROLLBACK_FILE_SIZE: usize = 5 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_format_handles_epoch_leap_day_and_milliseconds() {
        assert_eq!(format_unix_timestamp(0, None), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_unix_timestamp(1_582_934_400, Some(7)),
            "2020-02-29T00:00:00.007Z"
        );
    }

    #[test]
    fn generated_ids_are_unique_within_one_process() {
        let ids: std::collections::HashSet<_> =
            (0..10_000).map(|_| generate_session_id()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn deleting_a_missing_session_is_reported() {
        let id = format!("missing-{}", generate_session_id());
        let error = delete_session(&id).unwrap_err().to_string();
        assert!(error.contains("was not found"));
    }

    #[test]
    fn session_timestamps_are_valid_utc_shapes() {
        let session = Session::new("test".into(), None);
        assert!(session.created_at.ends_with('Z'));
        assert_eq!(session.created_at.len(), 24);
        assert_eq!(session.created_at.as_bytes()[10], b'T');
    }
}
