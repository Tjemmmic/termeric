//! Session restoration functionality

use std::fs;
use std::path::Path;
use anyhow::Result;

use super::{Session, session_file_path};

/// Restore a session from the default location
pub fn restore_session() -> Result<Option<Session>> {
    let path = session_file_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine session file path"))?;

    if !path.exists() {
        return Ok(None);
    }

    restore_session_from_path(&path).map(Some)
}

/// Restore a session from a specific path
pub fn restore_session_from_path(path: &Path) -> Result<Session> {
    let content = fs::read_to_string(path)?;
    let session: Session = toml::from_str(&content)?;

    if !session.is_compatible() {
        anyhow::bail!(
            "Session version {} is not compatible with current version {}",
            session.version,
            Session::CURRENT_VERSION
        );
    }

    log::info!("Session restored from {:?}", path);
    Ok(session)
}

/// Delete the saved session file
pub fn clear_session() -> Result<()> {
    if let Some(path) = session_file_path() {
        if path.exists() {
            fs::remove_file(&path)?;
            log::info!("Session file deleted");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SavedTab, save::save_session_to_path};
    use tempfile::tempdir;

    #[test]
    fn test_restore_session() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test_session.toml");

        // Create and save a session
        let mut session = Session::new();
        session.tabs.push(SavedTab::new(
            "restored".to_string(),
            Some("/home/test".to_string()),
        ));
        session.active_tab = 0;

        save_session_to_path(&session, &path).unwrap();

        // Restore and verify
        let restored = restore_session_from_path(&path).unwrap();
        assert_eq!(restored.version, Session::CURRENT_VERSION);
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.tabs[0].title, "restored");
        assert_eq!(
            restored.tabs[0].working_directory,
            Some("/home/test".to_string())
        );
    }

    #[test]
    fn test_restore_nonexistent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");

        let result = restore_session_from_path(&path);
        assert!(result.is_err());
    }
}
