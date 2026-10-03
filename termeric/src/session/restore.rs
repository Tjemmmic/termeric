//! Session restoration functionality

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use super::{Session, session_dir};
use crate::safe_fs::read_text;

/// Restore a named session by its ID
pub fn restore_named_session(name: &str) -> Result<Option<Session>> {
    let dir = session_dir(name)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;

    let session_file = dir.join("session.toml");
    match fs::symlink_metadata(&session_file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(_) => {}
    }

    restore_session_from_path(&session_file).map(Some)
}

/// Restore a session from a specific path
pub fn restore_session_from_path(path: &Path) -> Result<Session> {
    let session = super::snapshot::read_metadata(path)?;

    log::info!("Restored session '{}' from {:?}", session.id, path);
    Ok(session)
}

/// Validate a selection before spawning a shell or changing tab state.
pub fn load_snapshot_scrollback(session: &Session) -> Result<Vec<String>> {
    if session.version >= 5 {
        let dir = session_dir(&session.id).context("Invalid session directory")?;
        super::snapshot::read_selected(&dir.join("session.toml"), session)
    } else if let Some(filename) = &session.scrollback_file {
        load_scrollback_text(&session.id, filename)
    } else {
        Ok(Vec::new())
    }
}

/// Load scrollback text for a session from its directory
///
/// Returns the lines of scrollback text. Missing or unreadable data is an
/// error so callers cannot mistake partial recovery for a complete restore.
pub fn load_scrollback_text(session_id: &str, scrollback_file: &str) -> Result<Vec<String>> {
    let filename = Path::new(scrollback_file);
    if filename.components().count() != 1 || filename.file_name().is_none() {
        anyhow::bail!("Invalid scrollback filename: {scrollback_file:?}");
    }
    let dir = session_dir(session_id)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;

    let filepath = dir.join(scrollback_file);
    let content = read_text(&filepath, super::MAX_SCROLLBACK_FILE_SIZE)
        .with_context(|| format!("read scrollback from {}", filepath.display()))?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    log::info!(
        "Loaded {} scrollback lines from {:?}",
        lines.len(),
        filepath
    );
    Ok(lines)
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
        assert_eq!(
            session.capture_scope,
            super::super::SessionCaptureScope::SingleTerminal
        );
    }

    #[test]
    fn legacy_v1_and_v3_fixtures_remain_compatible() {
        for fixture in [
            include_str!("../../tests/fixtures/sessions/v1.toml"),
            include_str!("../../tests/fixtures/sessions/v3.toml"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("session.toml");
            fs::write(&path, fixture).unwrap();
            let session = restore_session_from_path(&path).unwrap();
            assert!(session.is_compatible());
            assert_eq!(
                session.capture_scope,
                super::super::SessionCaptureScope::SingleTerminal
            );
        }
    }

    #[test]
    fn legacy_v4_fixture_records_focused_pane_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.toml");
        fs::write(
            &path,
            include_str!("../../tests/fixtures/sessions/v4-focused-pane.toml"),
        )
        .unwrap();
        let session = restore_session_from_path(&path).unwrap();
        assert_eq!(session.version, 4);
        assert_eq!(
            session.capture_scope,
            super::super::SessionCaptureScope::FocusedPane
        );
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

    #[test]
    fn corrupt_and_incompatible_metadata_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let corrupt = dir.path().join("corrupt.toml");
        fs::write(&corrupt, "not = [valid").unwrap();
        assert!(restore_session_from_path(&corrupt).is_err());

        let newer = dir.path().join("newer.toml");
        fs::write(
            &newer,
            r#"version = 999
id = "future"
title = "future"
created_at = "2020-01-01T00:00:00Z"
updated_at = "2020-01-01T00:00:00Z"
"#,
        )
        .unwrap();
        let error = restore_session_from_path(&newer).unwrap_err().to_string();
        assert!(error.contains("newer than supported"));
    }

    #[test]
    fn scrollback_paths_cannot_escape_the_session_directory() {
        let error = load_scrollback_text("valid-session", "../outside.txt")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Invalid scrollback filename"));
    }

    #[test]
    fn production_scrollback_restore_enforces_the_writer_byte_limit() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("fixture");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("scrollback.txt");
        fs::write(&path, "界\nlast line\n").unwrap();
        super::super::set_test_sessions_base_dir(Some(root.path().to_owned()));
        let valid = load_scrollback_text("fixture", "scrollback.txt");
        fs::File::create(&path)
            .unwrap()
            .set_len(super::super::MAX_SCROLLBACK_FILE_SIZE as u64 + 1)
            .unwrap();
        let oversized = load_scrollback_text("fixture", "scrollback.txt");
        super::super::set_test_sessions_base_dir(None);
        assert_eq!(valid.unwrap(), ["界", "last line"]);
        assert!(format!("{:#}", oversized.unwrap_err()).contains("limit"));
    }

    #[cfg(unix)]
    #[test]
    fn named_restore_reports_a_broken_metadata_symlink_as_an_error() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("fixture");
        fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(dir.join("absent"), dir.join("session.toml")).unwrap();
        super::super::set_test_sessions_base_dir(Some(root.path().to_owned()));
        let result = restore_named_session("fixture");
        super::super::set_test_sessions_base_dir(None);
        assert!(result.is_err());
    }
}
