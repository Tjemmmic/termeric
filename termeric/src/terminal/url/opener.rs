//! Desktop opener children, reaped independently of GUI lifetimes.

use std::io;
use std::process::Command;
use std::sync::mpsc;
use std::thread::JoinHandle;

/// Starts `command` and returns once it has spawned. A detached waiter reaps
/// the child, even if its tab or window has gone by the time it exits.
pub(super) fn launch(command: Command) -> io::Result<()> {
    start(command).map(drop)
}

fn start(command: Command) -> io::Result<JoinHandle<()>> {
    let (started, result) = mpsc::sync_channel(1);
    // Create the waiter before spawning its child, so a thread creation
    // failure cannot leave a process nobody reaps.
    let worker = std::thread::Builder::new()
        .name("termeric-opener".into())
        .spawn(move || run(command, started))?;
    result
        .recv()
        .map_err(|_| io::Error::other("desktop opener worker stopped before launch"))??;
    Ok(worker)
}

fn run(mut command: Command, started: mpsc::SyncSender<io::Result<()>>) {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    // This acknowledges the spawn, not the desktop handler's acceptance.
    // Losing the receiver must not skip reaping.
    let _ = started.send(Ok(()));
    match child.wait() {
        Ok(status) if !status.success() => {
            log::warn!("Desktop opener exited unsuccessfully: {status}");
        }
        Err(error) => log::warn!("Could not reap desktop opener: {error}"),
        Ok(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    /// A child that records its PID, then waits for `release` to close.
    fn fixture(path: &std::path::Path) -> (Command, UnixStream) {
        let (input, release) = UnixStream::pair().unwrap();
        let mut command = Command::new("/bin/sh");
        // Only this fixed fixture script runs: no desktop or provider.
        command
            .args(["-c", "printf '%s' \"$$\" > \"$1\"; read line", "fixture"])
            .arg(path)
            .stdin(std::process::Stdio::from(OwnedFd::from(input)));
        (command, release)
    }

    fn assert_reaped(path: &std::path::Path) {
        let pid: libc::pid_t = std::fs::read_to_string(path).unwrap().parse().unwrap();
        let mut status = 0;
        // SAFETY: The PID names our completed fixture, status is writable,
        // and WNOHANG prevents waiting even if the regression recurs.
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        let error = io::Error::last_os_error();
        assert_eq!(result, -1, "fixture was not reaped by its worker");
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }

    #[test]
    fn failed_spawn_returns_the_original_error() {
        let directory = tempfile::tempdir().unwrap();
        let command = Command::new(directory.path().join("nonexistent-opener"));
        assert_eq!(start(command).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn start_returns_before_the_child_exits_and_the_child_is_reaped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid");
        let (command, release) = fixture(&path);
        let (sent, received) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = sent.send(start(command));
        });
        // A regression that waits for exit before acknowledging must fail,
        // not hang the test while the fixture waits for our EOF.
        let worker = received
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("spawn acknowledgement waited for child exit")
            .unwrap();
        drop(release);
        worker.join().unwrap();
        assert_reaped(&path);
    }

    #[test]
    fn dropped_acknowledgement_still_reaps_the_child() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pid");
        let (command, release) = fixture(&path);
        drop(release);
        let (started, result) = mpsc::sync_channel(1);
        drop(result);
        run(command, started);
        assert_reaped(&path);
    }
}
