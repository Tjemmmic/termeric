//! Tab title generation helpers
//!
//! Functions for deriving tab titles from process state (CWD, foreground process).

use std::path::Path;
use termeric_core::Terminal;

/// Get the current working directory of a process by its PID
#[cfg(target_os = "linux")]
pub(crate) fn get_process_cwd(pid: u32) -> Option<String> {
    let cwd_path = format!("/proc/{}/cwd", pid);
    std::fs::read_link(&cwd_path)
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn get_process_cwd(_pid: u32) -> Option<String> {
    None
}

/// Return a shell-reported directory only when its OSC 7 authority identifies
/// this machine. Remote hosts remain useful display metadata but must never be
/// treated as local filesystem paths for Git, persistence, or process spawn.
pub(crate) fn terminal_working_directory(terminal: &Terminal) -> Option<String> {
    local_reported_directory(terminal.reported_working_directory()?)
}

pub(crate) fn presentation_working_directory(terminal: &Terminal) -> Option<String> {
    local_reported_directory(terminal.presentation_working_directory()?)
}

fn local_reported_directory(
    directory: &termeric_core::term::ReportedWorkingDirectory,
) -> Option<String> {
    if !directory.host().is_none_or(host_is_local) {
        return None;
    }

    #[cfg(target_os = "windows")]
    if directory.path().as_bytes().get(0..4).is_some_and(|prefix| {
        prefix[0] == b'/'
            && prefix[1].is_ascii_alphabetic()
            && prefix[2] == b':'
            && prefix[3] == b'/'
    }) {
        return Some(directory.path()[1..].to_string());
    }

    Some(directory.path().to_string())
}

pub(crate) fn terminal_reported_directory_path(terminal: &Terminal) -> Option<&str> {
    terminal
        .reported_working_directory()
        .map(|directory| directory.path())
}

fn host_is_local(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    if ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|local| host.eq_ignore_ascii_case(local))
    {
        return true;
    }
    local_hostname()
        .as_deref()
        .map(|local| local.trim_end_matches('.'))
        .is_some_and(|local| host.eq_ignore_ascii_case(local))
}

#[cfg(unix)]
fn local_hostname() -> Option<String> {
    let mut bytes = [0u8; 256];
    // SAFETY: `bytes` is a writable buffer of the exact length supplied to
    // gethostname, and it remains alive for the duration of the call.
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return None;
    }
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..length])
        .ok()
        .filter(|hostname| !hostname.is_empty())
        .map(String::from)
}

#[cfg(target_os = "windows")]
fn local_hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|hostname| !hostname.is_empty())
}

#[cfg(not(any(unix, target_os = "windows")))]
fn local_hostname() -> Option<String> {
    None
}

/// Extract the directory name from a path, showing ~ for home directory
pub(crate) fn dir_name_from_path(path: &str) -> String {
    // Check if this is the home directory
    if let Some(home) = std::env::var_os("HOME") {
        if path == home.to_str().unwrap_or("") {
            return "~".to_string();
        }
    }

    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string())
}

/// Abbreviate a full path by replacing $HOME prefix with ~
pub(crate) fn abbreviate_path(path: &str) -> String {
    if let Some(home) = std::env::var_os("HOME") {
        if let Some(home_str) = home.to_str() {
            if path == home_str {
                return "~".to_string();
            }
            if let Some(rest) = path.strip_prefix(home_str) {
                return format!("~{}", rest);
            }
        }
    }
    path.to_string()
}

/// Get the name of the foreground process running in the shell
#[cfg(target_os = "linux")]
pub(crate) fn get_foreground_process(shell_pid: u32) -> Option<String> {
    // Read child processes from /proc/<pid>/task/<pid>/children
    let children_path = format!("/proc/{}/task/{}/children", shell_pid, shell_pid);
    let children = std::fs::read_to_string(&children_path).ok()?;
    let child_pid: u32 = children.split_whitespace().last()?.parse().ok()?;

    // Read the comm (command name) of the child process
    let comm_path = format!("/proc/{}/comm", child_pid);
    let comm = std::fs::read_to_string(&comm_path).ok()?;
    let name = comm.trim().to_string();

    // Don't return shell names as foreground processes
    if name.is_empty() || ["bash", "zsh", "sh", "fish"].contains(&name.as_str()) {
        return None;
    }
    Some(name)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn get_foreground_process(_shell_pid: u32) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presented_directory_preserves_absent_and_remote_snapshot_metadata() {
        for (initial, expected) in [
            ("", None),
            ("file://localhost/tmp/visible", Some("/tmp/visible")),
            ("file://remote.example/tmp/remote", None),
        ] {
            let mut terminal = Terminal::new(8, 2);
            if !initial.is_empty() {
                terminal.process(format!("\x1b]7;{initial}\x07").as_bytes());
            }
            let original = terminal.reported_working_directory().cloned();
            terminal.process(b"\x1b[?2026h\x1b]7;file:///tmp/hidden\x07");
            assert_eq!(terminal.presentation_working_directory(), original.as_ref());
            assert_eq!(
                presentation_working_directory(&terminal).as_deref(),
                expected
            );
            assert_eq!(
                terminal_working_directory(&terminal).as_deref(),
                Some("/tmp/hidden")
            );
            terminal.process(b"\x1b[?2026l");
            assert_eq!(
                presentation_working_directory(&terminal).as_deref(),
                Some("/tmp/hidden")
            );
        }
    }

    #[test]
    fn reported_directory_accepts_local_authorities_and_rejects_remote_hosts() {
        let mut terminal = Terminal::new(8, 2);
        terminal.process(b"\x1b]7;file:///tmp/local%20path\x07");
        assert_eq!(
            terminal_working_directory(&terminal).as_deref(),
            Some("/tmp/local path")
        );

        terminal.process(b"\x1b]7;file://localhost/tmp/loopback\x07");
        assert_eq!(
            terminal_working_directory(&terminal).as_deref(),
            Some("/tmp/loopback")
        );

        terminal.process(b"\x1b]7;file://remote.example/tmp/not-local\x07");
        assert_eq!(terminal_working_directory(&terminal), None);
        assert_eq!(
            terminal_reported_directory_path(&terminal),
            Some("/tmp/not-local")
        );
    }
}
