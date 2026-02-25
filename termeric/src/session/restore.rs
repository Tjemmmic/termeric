//! Session restoration functionality

use std::fs;
use std::path::Path;
use anyhow::Result;

use super::{Session, session_dir};

/// Restore a session from the default location (legacy, no longer used for startup)
pub fn restore_session() -> Result<Option<Session>> {
    // No longer auto-restoring on startup — always start fresh
    Ok(None)
}

/// Restore a named session by its ID
pub fn restore_named_session(name: &str) -> Result<Option<Session>> {
    let dir = session_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;

    let session_file = dir.join("session.toml");
    if !session_file.exists() {
        return Ok(None);
    }

    restore_session_from_path(&session_file).map(Some)
}

/// Restore a session from a specific path
pub fn restore_session_from_path(path: &Path) -> Result<Session> {
    let content = fs::read_to_string(path)?;
    let session: Session = toml::from_str(&content)?;

    if !session.is_compatible() {
        anyhow::bail!(
            "Session file version {} is newer than supported version {}",
            session.version,
            Session::CURRENT_VERSION,
        );
    }

    log::info!("Restored session '{}' from {:?}", session.id, path);
    Ok(session)
}

/// Load scrollback text for a session from its directory
///
/// Returns the lines of scrollback text, or an empty vec if the file
/// doesn't exist or can't be read.
pub fn load_scrollback_text(
    session_id: &str,
    scrollback_file: &str,
) -> Vec<String> {
    let Some(dir) = session_dir(session_id) else {
        return Vec::new();
    };

    let filepath = dir.join(scrollback_file);
    match fs::read_to_string(&filepath) {
        Ok(content) => {
            let lines: Vec<String> = content.lines().map(String::from).collect();
            log::info!("Loaded {} scrollback lines from {:?}", lines.len(), filepath);
            lines
        }
        Err(e) => {
            log::warn!("Could not load scrollback from {:?}: {}", filepath, e);
            Vec::new()
        }
    }
}

/// Clear the default session data (legacy cleanup)
pub fn clear_session() -> Result<()> {
    // No-op for the new per-tab model
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_restore_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        assert!(!path.exists());
    }

    #[test]
    fn test_restore_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");

        let content = r#"
version = 3
id = "2026-02-24_21h59m43"
title = "test"
working_directory = "/tmp"
created_at = "2026-02-24T21:59:43Z"
updated_at = "2026-02-24T21:59:43Z"
"#;

        let mut file = fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let session = restore_session_from_path(&path).unwrap();
        assert_eq!(session.version, 3);
        assert_eq!(session.id, "2026-02-24_21h59m43");
        assert_eq!(session.title, "test");
    }

    #[test]
    fn test_load_scrollback_text() {
        let dir = tempfile::tempdir().unwrap();
        let filepath = dir.path().join("scrollback.txt");

        let mut file = fs::File::create(&filepath).unwrap();
        writeln!(file, "$ ls").unwrap();
        writeln!(file, "file1.txt  file2.txt").unwrap();
        writeln!(file, "$ pwd").unwrap();
        writeln!(file, "/home/user").unwrap();

        let content = fs::read_to_string(&filepath).unwrap();
        let lines: Vec<String> = content.lines().map(String::from).collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], "$ ls");
        assert_eq!(lines[3], "/home/user");
    }
}
