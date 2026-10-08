//! What a terminal's processes report about themselves: the directory they
//! work in and the program in front, from the shell's own reports and from
//! `/proc`.

use termeric_core::Terminal;
use termeric_core::pty::PtyHandle;

/// Where a terminal's program works: the directory its shell last reported
/// for this machine, else its child process's current directory.
pub(crate) fn working_directory(terminal: &Terminal, pty: &PtyHandle) -> Option<String> {
    terminal_working_directory(terminal).or_else(|| pty.child_id().and_then(get_process_cwd))
}

/// Get the current working directory of a process by its PID
pub(crate) fn get_process_cwd(pid: u32) -> Option<String> {
    let cwd_path = format!("/proc/{}/cwd", pid);
    std::fs::read_link(&cwd_path)
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
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

    Some(directory.path().to_string())
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

/// Get the name of the foreground process running in the shell
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
    }
}
