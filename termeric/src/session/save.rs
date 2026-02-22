//! Session saving functionality

use std::fs;
use std::io::Write;
use std::path::Path;
use anyhow::Result;

use super::{Session, session_file_path};

/// Save a session to the default location
pub fn save_session(session: &Session) -> Result<()> {
    let path = session_file_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine session file path"))?;

    save_session_to_path(session, &path)
}

/// Save a session to a specific path
pub fn save_session_to_path(session: &Session, path: &Path) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Serialize to TOML
    let content = toml::to_string_pretty(session)?;

    // Write atomically by writing to a temp file first
    let temp_path = path.with_extension("toml.tmp");
    let mut file = fs::File::create(&temp_path)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;

    // Rename to final location
    fs::rename(&temp_path, path)?;

    log::info!("Session saved to {:?}", path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SavedTab;
    use tempfile::tempdir;

    #[test]
    fn test_save_session() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_session.toml");

        let mut session = Session::new();
        session.tabs.push(SavedTab::new(
            "test".to_string(),
            Some("/home/user".to_string()),
        ));

        save_session_to_path(&session, &path).unwrap();

        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("version = 1"));
        assert!(content.contains("/home/user"));
    }
}
