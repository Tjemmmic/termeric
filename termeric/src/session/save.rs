//! Session saving functionality

use anyhow::Result;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::path::Path;

#[cfg(test)]
use super::MAX_SCROLLBACK_FILE_SIZE;
#[cfg(test)]
use super::file_io::{MAX_SESSION_METADATA_BYTES, atomic_replace};
use super::{Session, session_dir};

/// Styled scrollback and screen rows to save with a session. Blank rows and
/// a final bare prompt, `exit`, or `logout` are dropped from the end, so the
/// restored shell's own prompt doesn't appear below a stale one.
pub fn restorable_scrollback(grid: &termeric_core::Grid) -> Vec<String> {
    let mut lines = grid.styled_lines();
    let first = -(grid.scrollback_len().min(i32::MAX as usize) as i32);
    let disposable = (first..grid.rows() as i32)
        .rev()
        .map_while(|offset| grid.row_at_offset(offset))
        .take_while(|row| ends_a_finished_session(row.plain_text(grid.cols()).trim()))
        .count();
    lines.truncate(lines.len().saturating_sub(disposable));
    lines
}

fn ends_a_finished_session(line: &str) -> bool {
    line.is_empty() || line == "logout" || line == "exit" || is_bare_prompt(line)
}

/// A short line ending in a common prompt character, like `$`, `[~]$`,
/// `user@host:~$`, `#`, `%`, or `>`; longer lines are likely output.
fn is_bare_prompt(line: &str) -> bool {
    line.len() <= 120 && line.ends_with(['$', '#', '%', '>'])
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub struct ScrollbackSave {
    pub filename: String,
    pub bytes_written: usize,
    pub truncated: bool,
}

/// Publish metadata and captured text together; return whether text was capped.
pub fn save_snapshot(session: &Session, lines: &[String]) -> Result<bool> {
    let dir = session_dir(&session.id).ok_or_else(|| {
        anyhow::anyhow!("Could not determine session directory for '{}'", session.id)
    })?;

    super::snapshot::save(session, &dir, lines)
}

/// Save a session to a specific directory
///
/// Uses an atomic write pattern: writes to a temp file, syncs, then renames.
#[cfg(test)]
pub fn save_session_to_dir(session: &Session, dir: &Path) -> Result<()> {
    super::snapshot::save(session, dir, &[])?;
    Ok(())
}

/// Legacy fixture writer; application saves use the v5 transaction above.
#[cfg(test)]
fn save_scrollback_text(session_id: &str, lines: &[String]) -> Result<ScrollbackSave> {
    let dir = session_dir(session_id)
        .ok_or_else(|| anyhow::anyhow!("Could not determine session directory"))?;
    fs::create_dir_all(&dir)?;
    let filepath = dir.join("scrollback.txt");
    let report = save_scrollback_to_path(&filepath, lines, MAX_SCROLLBACK_FILE_SIZE)?;
    if report.truncated {
        log::warn!(
            "Scrollback for session '{}' truncated at {} bytes (cap: {} bytes)",
            session_id,
            report.bytes_written,
            MAX_SCROLLBACK_FILE_SIZE
        );
    }
    log::info!(
        "Saved scrollback for session '{}' ({} bytes) to {:?}",
        session_id,
        report.bytes_written,
        filepath
    );
    Ok(report)
}

/// Preserve the legacy per-file regression fixtures. V5's production whole-line
/// cap and transaction boundaries are tested separately in `snapshot`.
#[cfg(test)]
fn save_scrollback_to_path(
    filepath: &Path,
    lines: &[String],
    max_size: usize,
) -> Result<ScrollbackSave> {
    let filename = filepath
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("scrollback path has no filename"))?
        .to_string_lossy()
        .into_owned();
    Ok(atomic_replace(filepath, |file| {
        let mut bytes_written: usize = 0;
        let mut truncated = false;
        for line in lines {
            let needed = line.len().saturating_add(1);
            if bytes_written.saturating_add(needed) > max_size {
                truncated = true;
                break;
            }
            writeln!(file, "{line}")?;
            bytes_written += needed;
        }
        Ok(ScrollbackSave {
            filename,
            bytes_written,
            truncated,
        })
    })?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines_after(output: &[u8]) -> Vec<String> {
        let mut terminal = termeric_core::Terminal::new(20, 4);
        terminal.process(output);
        restorable_scrollback(terminal.grid())
    }

    #[test]
    fn restorable_scrollback_drops_a_trailing_prompt_exit_and_blank_rows() {
        let saved = lines_after(b"one\r\n\x1b[31mtwo\x1b[0m\r\nexit\r\nuser@host:~$ ");
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0], "one");
        assert!(saved[1].contains("\x1b[") && saved[1].contains("two"));
    }

    #[test]
    fn restorable_scrollback_keeps_output_that_merely_ends_like_a_prompt() {
        let long = format!("{}$", "x".repeat(130));
        let mut terminal = termeric_core::Terminal::new(200, 3);
        terminal.process(format!("{long}\r\nlast line\r\n$ ").as_bytes());
        assert_eq!(
            restorable_scrollback(terminal.grid()),
            [long, "last line".into()]
        );
        assert!(lines_after(b"$ \r\n#").is_empty());
    }
    use crate::session::Session;
    use std::io::Write;

    #[cfg(unix)]
    #[test]
    fn legacy_metadata_staging_symlink_does_not_modify_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("private-sentinel");
        fs::write(&target, "must remain unchanged").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("session.toml.tmp")).unwrap();
        save_session_to_dir(&Session::new("new snapshot".into(), None), dir.path()).unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "must remain unchanged");
    }

    #[cfg(unix)]
    #[test]
    fn legacy_scrollback_destination_symlink_does_not_modify_target() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("fixture");
        fs::create_dir(&dir).unwrap();
        let target = root.path().join("private-sentinel");
        fs::write(&target, "must remain unchanged").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("scrollback.txt")).unwrap();
        crate::session::set_test_sessions_base_dir(Some(root.path().to_owned()));
        let result = save_scrollback_text("fixture", &["new text".into()]);
        crate::session::set_test_sessions_base_dir(None);
        result.unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "must remain unchanged");
    }

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
    fn scrollback_cap_is_enforced_without_partial_or_duplicate_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrollback.txt");
        let line = "x".repeat(8_191);
        let lines = vec![line; MAX_SCROLLBACK_FILE_SIZE / 8_192 + 1];
        let report = save_scrollback_to_path(&path, &lines, MAX_SCROLLBACK_FILE_SIZE).unwrap();
        assert_eq!(report.bytes_written, MAX_SCROLLBACK_FILE_SIZE);
        assert!(report.truncated);
        let bytes = fs::read(path).unwrap();
        assert_eq!(bytes.len(), MAX_SCROLLBACK_FILE_SIZE);
        assert_eq!(bytes.iter().filter(|&&byte| byte == b'\n').count(), 640);
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

    #[test]
    fn oversized_metadata_save_preserves_the_previous_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::new("original".into(), None);
        save_session_to_dir(&session, dir.path()).unwrap();
        let previous = fs::read(dir.path().join("session.toml")).unwrap();
        session.title = "x".repeat(MAX_SESSION_METADATA_BYTES);
        assert!(
            save_session_to_dir(&session, dir.path())
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert_eq!(fs::read(dir.path().join("session.toml")).unwrap(), previous);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
