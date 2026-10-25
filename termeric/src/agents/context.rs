//! Gathering bounded context for an agent: Git diffs, the file tree, terminal
//! and test output, and chosen files, each capped and run off the UI thread.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::ui::context_picker::ContextSourceId;

pub const MAX_CONTEXT_FILES: usize = 16;
pub const MAX_CONTEXT_PATH_CHARS: usize = 1024;
pub const MAX_CONTEXT_SOURCE_CHARS: usize = 10_000;
pub const MAX_ASSEMBLED_CONTEXT_CHARS: usize = 64_000;
/// The widest a UTF-8 character can be.
const MAX_UTF8_CHAR_BYTES: usize = 4;
/// The most bytes [`MAX_CONTEXT_SOURCE_CHARS`] characters can take.
const MAX_CONTEXT_SOURCE_BYTES: usize = MAX_CONTEXT_SOURCE_CHARS * MAX_UTF8_CHAR_BYTES;
/// How many files a directory walk lists when Git cannot list them.
const MAX_FILE_TREE_ENTRIES: usize = 5_000;
/// How deep a directory walk goes when Git cannot list the files.
const MAX_FILE_TREE_DEPTH: usize = 32;
const CONTEXT_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextGatherRequest {
    pub request_id: u64,
    pub cwd: Option<PathBuf>,
    pub selected: Vec<ContextSourceId>,
    pub specific_files: Vec<PathBuf>,
    pub terminal_output: Option<String>,
}

pub(crate) fn is_git_repo(cwd: Option<&Path>) -> bool {
    cwd.and_then(|path| path.canonicalize().ok())
        .is_some_and(|path| {
            path.ancestors()
                .any(|ancestor| ancestor.join(".git").exists())
        })
}

pub(crate) fn validate_specific_files(
    cwd: Option<&Path>,
    value: &str,
) -> Result<Vec<PathBuf>, String> {
    let root = context_root(cwd)?;
    let entries: Vec<&Path> = value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(Path::new)
        .collect();
    validate_specific_paths(&root, &entries)
}

/// Check files chosen as paths (the desktop's file chooser), as
/// [`validate_specific_files`] checks typed ones: each an existing regular
/// file inside `cwd`, at most [`MAX_CONTEXT_FILES`] of them.
pub(crate) fn validate_chosen_files(
    cwd: Option<&Path>,
    chosen: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let root = context_root(cwd)?;
    let entries: Vec<&Path> = chosen.iter().map(PathBuf::as_path).collect();
    validate_specific_paths(&root, &entries)
}

/// The folder specific files must be inside, resolved.
fn context_root(cwd: Option<&Path>) -> Result<PathBuf, String> {
    cwd.ok_or_else(|| "Specific files require a working directory".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not resolve the working directory: {error}"))
}

fn validate_specific_paths(root: &Path, entries: &[&Path]) -> Result<Vec<PathBuf>, String> {
    if entries.len() > MAX_CONTEXT_FILES {
        return Err(format!("Choose at most {MAX_CONTEXT_FILES} files"));
    }
    let mut paths = Vec::with_capacity(entries.len());
    for &candidate in entries {
        let entry = candidate.display();
        if entry.to_string().chars().count() > MAX_CONTEXT_PATH_CHARS {
            return Err(format!(
                "A file path exceeds {MAX_CONTEXT_PATH_CHARS} characters"
            ));
        }
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            root.join(candidate)
        };
        let canonical = candidate
            .canonicalize()
            .map_err(|error| format!("Could not open {entry}: {error}"))?;
        if !canonical.starts_with(root) {
            return Err(format!("{entry} is outside the context working directory"));
        }
        if !canonical.is_file() {
            return Err(format!("{entry} is not a regular file"));
        }
        if !paths.contains(&canonical) {
            paths.push(canonical);
        }
    }
    Ok(paths)
}

pub fn gather_context(request: ContextGatherRequest) -> Result<String, String> {
    let mut parts = Vec::new();
    for source in request.selected {
        let part = match source {
            ContextSourceId::GitDiffUnstaged => bounded_git_diff(request.cwd.as_deref(), false)?
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!("=== Git Diff (Unstaged) ===\n{text}")),
            ContextSourceId::GitDiffStaged => bounded_git_diff(request.cwd.as_deref(), true)?
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!("=== Git Diff (Staged) ===\n{text}")),
            ContextSourceId::FileTree => bounded_file_tree(request.cwd.as_deref())?
                .filter(|text| !text.trim().is_empty())
                .map(|text| format!("=== File Tree ===\n{text}")),
            ContextSourceId::TerminalOutput => request
                .terminal_output
                .as_deref()
                .filter(|text| !text.trim().is_empty())
                .map(|text| {
                    format!(
                        "=== Recent Terminal Output ===\n{}",
                        crate::safe_text::truncate_with_ellipsis(text, MAX_CONTEXT_SOURCE_CHARS)
                    )
                }),
            ContextSourceId::TestOutput => request
                .terminal_output
                .as_deref()
                .and_then(extract_test_output)
                .map(|text| {
                    format!(
                        "=== Test Output ===\n{}",
                        crate::safe_text::truncate_with_ellipsis(&text, MAX_CONTEXT_SOURCE_CHARS)
                    )
                }),
            ContextSourceId::SpecificFiles => {
                let files = gather_specific_files(request.cwd.as_deref(), &request.specific_files)?;
                (!files.is_empty()).then(|| files.join("\n\n"))
            }
        };
        if let Some(part) = part {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return Err("Selected sources produced no context".into());
    }
    Ok(crate::safe_text::truncate_with_ellipsis(
        &parts.join("\n\n"),
        MAX_ASSEMBLED_CONTEXT_CHARS,
    ))
}

fn gather_specific_files(root: Option<&Path>, paths: &[PathBuf]) -> Result<Vec<String>, String> {
    let root = root
        .ok_or_else(|| "Specific files require a working directory".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not resolve the working directory: {error}"))?;
    let mut files = Vec::with_capacity(paths.len().min(MAX_CONTEXT_FILES));
    for path in paths.iter().take(MAX_CONTEXT_FILES) {
        let canonical = path
            .canonicalize()
            .map_err(|error| format!("Could not resolve {}: {error}", path.display()))?;
        if !canonical.starts_with(&root) || !canonical.is_file() {
            return Err(format!(
                "{} is no longer a regular file inside the context working directory",
                path.display()
            ));
        }
        let content = read_file_start(&canonical, MAX_CONTEXT_SOURCE_CHARS)
            .map_err(|error| format!("Could not read {}: {error}", canonical.display()))?;
        files.push(format!("=== File: {} ===\n{content}", canonical.display()));
    }
    Ok(files)
}

/// At most `max_chars` characters from the start of `path`, cut as
/// [`crate::safe_text::truncate_with_ellipsis`] cuts them. Only the bytes
/// those characters can take are read, however large the file is.
fn read_file_start(path: &Path, max_chars: usize) -> std::io::Result<String> {
    // Room for one character past the cap, so a longer file still reads as
    // cut and ends in an ellipsis.
    let limit = (max_chars + 1) * MAX_UTF8_CHAR_BYTES;
    let mut bytes = Vec::new();
    // The file was checked when it was chosen, but it may since have been
    // swapped for a FIFO, a device, or a link: the descriptor is what must be
    // a regular file. Its size does not matter, since only its start is read.
    crate::safe_fs::open_regular(path, usize::MAX)?
        .take(limit as u64)
        .read_to_end(&mut bytes)?;
    let reached_limit = bytes.len() == limit;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        // The limit cut the last character short; the file may well go on.
        Err(error) if reached_limit && error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).expect("the prefix before the cut is valid UTF-8")
        }
        Err(error) => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    };
    Ok(crate::safe_text::truncate_with_ellipsis(&text, max_chars))
}

fn bounded_git_diff(cwd: Option<&Path>, staged: bool) -> Result<Option<String>, String> {
    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    run_bounded_command("git", &args, cwd, MAX_CONTEXT_SOURCE_BYTES)
}

fn bounded_file_tree(cwd: Option<&Path>) -> Result<Option<String>, String> {
    if let Some(output) = run_bounded_command("git", &["ls-files"], cwd, MAX_CONTEXT_SOURCE_BYTES)?
    {
        if !output.trim().is_empty() {
            return Ok(Some(crate::safe_text::truncate_with_ellipsis(
                &output,
                MAX_CONTEXT_SOURCE_CHARS,
            )));
        }
    }
    let Some(root) = cwd else {
        return Ok(None);
    };
    let mut files = Vec::new();
    collect_files(root, root, 0, &mut files)?;
    Ok((!files.is_empty()).then(|| {
        crate::safe_text::truncate_with_ellipsis(&files.join("\n"), MAX_CONTEXT_SOURCE_CHARS)
    }))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    depth: usize,
    output: &mut Vec<String>,
) -> Result<(), String> {
    if depth > MAX_FILE_TREE_DEPTH || output.len() >= MAX_FILE_TREE_ENTRIES {
        return Ok(());
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("Could not list {}: {error}", directory.display()))?;
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Could not read directory entry: {error}"))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if output.len() >= MAX_FILE_TREE_ENTRIES {
            break;
        }
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(root, &path, depth + 1, output)?;
        } else if kind.is_file() {
            output.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn run_bounded_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not start {program}: {error}")),
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("Could not capture {program} output"))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + CONTEXT_COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("{program} context command timed out"));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("Could not wait for {program}: {error}"));
            }
        }
    };
    let mut bytes = reader
        .join()
        .map_err(|_| format!("{program} output reader panicked"))?
        .map_err(|error| format!("Could not read {program} output: {error}"))?;
    let truncated = bytes.len() > max_bytes;
    bytes.truncate(max_bytes);
    if !status.success() && !truncated {
        return Ok(None);
    }
    let mut output = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        output.push_str("\n… output truncated");
    }
    Ok(Some(output))
}

pub fn extract_test_output(output: &str) -> Option<String> {
    let patterns = [
        "test result:",
        "PASS",
        "FAIL",
        "panicked at",
        "error[E",
        "PASSED",
        "FAILED",
        "tests passed",
        "tests failed",
    ];
    let lines: Vec<_> = output.lines().collect();
    let mut selected = Vec::new();
    let mut last_match = None;
    for (index, line) in lines.iter().enumerate() {
        if patterns.iter().any(|pattern| line.contains(pattern)) {
            let start = index.saturating_sub(5);
            if last_match.is_none() {
                selected.extend_from_slice(&lines[start..index]);
            }
            selected.push(*line);
            last_match = Some(index);
        } else if last_match.is_some_and(|last| index.saturating_sub(last) < 3) {
            selected.push(*line);
        } else {
            last_match = None;
        }
    }
    (!selected.is_empty()).then(|| selected.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specific_files_are_workspace_bounded_and_gathered_with_total_caps() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("fixture.rs");
        fs::write(&file, "x".repeat(MAX_CONTEXT_SOURCE_CHARS + 100)).unwrap();
        let paths = validate_specific_files(Some(directory.path()), "fixture.rs").unwrap();
        assert_eq!(paths, [file.canonicalize().unwrap()]);
        assert!(validate_specific_files(Some(directory.path()), "/etc/passwd").is_err());

        let text = gather_context(ContextGatherRequest {
            request_id: 1,
            cwd: Some(directory.path().to_path_buf()),
            selected: vec![ContextSourceId::SpecificFiles],
            specific_files: paths,
            terminal_output: None,
        })
        .unwrap();
        assert!(text.contains("=== File:"));
        assert!(text.chars().count() <= MAX_ASSEMBLED_CONTEXT_CHARS);
    }

    #[test]
    fn specific_files_are_read_only_as_far_as_the_cap() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("large.log");
        // Two-byte characters past the cap, then bytes that are not UTF-8:
        // reading the whole file would fail on them.
        let mut content = "é".repeat(MAX_CONTEXT_SOURCE_CHARS * 3).into_bytes();
        content.extend_from_slice(&[0xff; 64]);
        fs::write(&file, content).unwrap();
        let paths = validate_specific_files(Some(directory.path()), "large.log").unwrap();

        let text = gather_context(ContextGatherRequest {
            request_id: 3,
            cwd: Some(directory.path().to_path_buf()),
            selected: vec![ContextSourceId::SpecificFiles],
            specific_files: paths,
            terminal_output: None,
        })
        .unwrap();
        let (_, start) = text.split_once("===\n").unwrap();
        assert_eq!(start.chars().count(), MAX_CONTEXT_SOURCE_CHARS);
        assert!(start.ends_with('…'));
        assert!(
            start
                .chars()
                .rev()
                .skip(1)
                .all(|character| character == 'é')
        );
    }

    #[test]
    fn specific_files_cut_mid_character_by_the_bound_still_read() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("wide.txt");
        // Three-byte characters never land on the four-byte bound.
        fs::write(&file, "界".repeat(MAX_CONTEXT_SOURCE_CHARS * 2)).unwrap();
        let start = read_file_start(&file, MAX_CONTEXT_SOURCE_CHARS).unwrap();
        assert_eq!(start.chars().count(), MAX_CONTEXT_SOURCE_CHARS);
        assert!(start.ends_with('…'));
    }

    #[test]
    fn specific_files_that_are_short_keep_their_text_and_reject_invalid_utf8() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("short.txt");
        fs::write(&file, "fn main() {}\n").unwrap();
        assert_eq!(
            read_file_start(&file, MAX_CONTEXT_SOURCE_CHARS).unwrap(),
            "fn main() {}\n"
        );
        // A short file ending in half a character is not UTF-8.
        fs::write(&file, [b'a', 0xc3]).unwrap();
        let error = read_file_start(&file, MAX_CONTEXT_SOURCE_CHARS).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_file_swapped_for_a_fifo_is_refused_without_waiting_for_a_writer() {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        use std::sync::mpsc;
        let directory = tempfile::tempdir().unwrap();
        let fifo = directory.path().join("chosen.log");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: name is a live C string inside this private fixture directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let source = fifo.clone();
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            send.send(read_file_start(&source, MAX_CONTEXT_SOURCE_CHARS))
                .unwrap();
        });
        // A read that waits is released by a writer, so the test fails rather
        // than hangs.
        let mut rescued = false;
        let result = match receive.recv_timeout(Duration::from_millis(300)) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                rescued = true;
                drop(
                    fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(&fifo)
                        .unwrap(),
                );
                receive.recv_timeout(Duration::from_secs(2)).unwrap()
            }
            Err(error) => panic!("context worker failed: {error}"),
        };
        worker.join().unwrap();
        assert!(!rescued, "reading the chosen file waited for a FIFO writer");
        assert!(result.is_err());
    }

    #[test]
    fn specific_file_is_revalidated_after_selection_to_block_symlink_retargeting() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("fixture.rs");
        fs::write(&file, "safe fixture").unwrap();
        let paths = validate_specific_files(Some(directory.path()), "fixture.rs").unwrap();
        fs::remove_file(&file).unwrap();
        symlink("/etc/passwd", &file).unwrap();

        let result = gather_context(ContextGatherRequest {
            request_id: 2,
            cwd: Some(directory.path().to_path_buf()),
            selected: vec![ContextSourceId::SpecificFiles],
            specific_files: paths,
            terminal_output: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_output_extraction_is_bounded_to_relevant_blocks() {
        let output = "some random output\ncompiling...\ntest foo ... ok\ntest bar ... FAIL\ntest result: 1 passed; 1 failed\nmore stuff";
        let extracted = extract_test_output(output).unwrap();
        assert!(extracted.contains("FAIL"));
        assert!(extracted.contains("test result:"));
        assert!(extract_test_output("hello world").is_none());
    }
}
