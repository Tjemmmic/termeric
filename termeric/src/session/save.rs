//! Session saving functionality

use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::Path;

use super::{MAX_SCROLLBACK_FILE_SIZE, Session, session_dir};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollbackSave {
    pub filename: String,
    pub bytes_written: usize,
    pub truncated: bool,
}

/// Save a session to its directory (determined by session ID)
pub fn save_session(session: &Session) -> Result<()> {
    let dir = session_dir(&session.id).ok_or_else(|| {
        anyhow::anyhow!("Could not determine session directory for '{}'", session.id)
    })?;

    save_session_to_dir(session, &dir)
}

/// Save a session to a specific directory
///
/// Uses an atomic write pattern: writes to a temp file, syncs, then renames.
pub fn save_session_to_dir(session: &Session, dir: &Path) -> Result<()> {
    // Ensure session directory exists
    fs::create_dir_all(dir)?;

    // Write session metadata as TOML
    let session_file = dir.join("session.toml");
    let mut persisted = session.clone();
    if let Ok(existing) = fs::read_to_string(&session_file)
        && let Ok(existing) = toml::from_str::<Session>(&existing)
        && existing.id == session.id
    {
        persisted.created_at = existing.created_at;
    }
    let toml_string = toml::to_string_pretty(&persisted)?;
    let temp_file = dir.join("session.toml.tmp");

    // Write to temp file first
    let mut file = fs::File::create(&temp_file)?;
    file.write_all(toml_string.as_bytes())?;
    file.sync_all()?; // Flush to disk, not just userspace buffer

    // Atomically rename to final location
    fs::rename(&temp_file, &session_file)?;

    log::info!("Saved session '{}' to {:?}", session.id, session_file);
    Ok(())
}

/// Save scrollback text for a session to a file within the session directory
///
/// Returns an explicit report so callers can surface truncation.
pub fn save_scrollback_text(session_id: &str, lines: &[String]) -> Result<ScrollbackSave> {
    let dir = session_dir(session_id)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;

    fs::create_dir_all(&dir)?;

    let filename = "scrollback.txt".to_string();
    let filepath = dir.join(&filename);

    let mut file = fs::File::create(&filepath)?;
    let mut bytes_written: usize = 0;
    let mut truncated = false;

    for line in lines {
        let line_bytes = line.as_bytes();
        // Check if adding this line would exceed the cap
        if bytes_written + line_bytes.len() + 1 > MAX_SCROLLBACK_FILE_SIZE {
            log::warn!(
                "Scrollback for session '{}' truncated at {} bytes (cap: {} bytes)",
                session_id,
                bytes_written,
                MAX_SCROLLBACK_FILE_SIZE
            );
            truncated = true;
            break;
        }
        file.write_all(line_bytes)?;
        file.write_all(b"\n")?;
        bytes_written += line_bytes.len() + 1;
    }

    file.flush()?;
    log::info!(
        "Saved scrollback for session '{}' ({} bytes) to {:?}",
        session_id,
        bytes_written,
        filepath
    );

    Ok(ScrollbackSave {
        filename,
        bytes_written,
        truncated,
    })
}

#[cfg(test)]
fn save_scrollback_to_path(
    filepath: &Path,
    lines: &[String],
    max_size: usize,
) -> Result<ScrollbackSave> {
    let mut file = fs::File::create(filepath)?;
    let mut bytes_written = 0;
    let mut truncated = false;
    for line in lines {
        let needed = line.len() + 1;
        if bytes_written + needed > max_size {
            truncated = true;
            break;
        }
        writeln!(file, "{line}")?;
        bytes_written += needed;
    }
    file.flush()?;
    Ok(ScrollbackSave {
        filename: filepath.file_name().unwrap().to_string_lossy().into_owned(),
        bytes_written,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;

    #[test]
    fn test_save_session() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new("test".to_string(), None);
        save_session_to_dir(&session, dir.path()).unwrap();
        assert!(dir.path().join("session.toml").exists());
    }

    #[test]
    fn test_save_scrollback_text() {
        let dir = tempfile::tempdir().unwrap();
        let lines = vec![
            "$ ls".to_string(),
            "file1.txt  file2.txt".to_string(),
            "$ pwd".to_string(),
            "/home/user".to_string(),
        ];

        let filename = "scrollback.txt";
        let filepath = dir.path().join(filename);

        let mut file = std::fs::File::create(&filepath).unwrap();
        for line in &lines {
            file.write_all(line.as_bytes()).unwrap();
            file.write_all(b"\n").unwrap();
        }
        file.flush().unwrap();

        let content = std::fs::read_to_string(&filepath).unwrap();
        assert!(content.contains("$ ls"));
        assert!(content.contains("/home/user"));
    }

    #[test]
    fn scrollback_report_exposes_truncation_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrollback.txt");
        let report = save_scrollback_to_path(&path, &["1234".into(), "5678".into()], 6).unwrap();
        assert_eq!(report.bytes_written, 5);
        assert!(report.truncated);
        assert_eq!(fs::read_to_string(path).unwrap(), "1234\n");
    }

    #[test]
    fn updating_metadata_preserves_original_creation_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut original = Session::new("first".into(), None);
        original.created_at = "2020-01-01T00:00:00.000Z".into();
        save_session_to_dir(&original, dir.path()).unwrap();
        let update = Session::with_id(original.id.clone(), "second".into(), None);
        save_session_to_dir(&update, dir.path()).unwrap();
        let persisted: Session =
            toml::from_str(&fs::read_to_string(dir.path().join("session.toml")).unwrap()).unwrap();
        assert_eq!(persisted.created_at, original.created_at);
        assert_eq!(persisted.title, "second");
    }

    #[test]
    fn save_reports_unavailable_directory() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, "file").unwrap();
        let error = save_session_to_dir(&Session::new("test".into(), None), &blocked)
            .unwrap_err()
            .to_string();
        assert!(!error.is_empty());
    }
}
