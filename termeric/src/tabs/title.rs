//! Tab title generation helpers
//!
//! Functions for deriving tab titles from process state (CWD, foreground process).

use std::path::Path;

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
