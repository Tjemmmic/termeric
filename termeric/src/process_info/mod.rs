//! What the operating system says about another process of this user: its
//! working directory, parent, newest child, name, and arguments. Linux reads
//! them from `/proc`; macOS asks the kernel through libproc and `sysctl`.
//! Every answer is a snapshot that can be stale by the time it is used, and
//! a process that has ended, or that the system won't describe, answers
//! `None`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

use std::path::PathBuf;

/// The directory `pid` works in.
pub(crate) fn working_directory(pid: u32) -> Option<PathBuf> {
    platform::working_directory(pid)
}

/// The process that started `pid`.
pub(crate) fn parent(pid: u32) -> Option<u32> {
    platform::parent(pid)
}

/// The most recently started child of `pid`.
pub(crate) fn newest_child(pid: u32) -> Option<u32> {
    platform::newest_child(pid)
}

/// The short name the system gives `pid`'s program, without its directory.
pub(crate) fn name(pid: u32) -> Option<String> {
    platform::name(pid).filter(|name| !name.is_empty())
}

/// The argument vector `pid` was started with, its program first.
pub(crate) fn arguments(pid: u32) -> Option<Vec<String>> {
    platform::arguments(pid).filter(|arguments| !arguments.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    /// A `sleep` started from a directory of the test's own, ended on drop.
    struct Sleeper {
        child: Child,
        directory: tempfile::TempDir,
    }

    impl Sleeper {
        fn start() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let child = Command::new("/bin/sleep")
                .arg("30")
                .current_dir(directory.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            Self { child, directory }
        }
    }

    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn a_child_reports_its_directory_parent_name_and_arguments() {
        let sleeper = Sleeper::start();
        let pid = sleeper.child.id();
        assert_eq!(
            working_directory(pid).map(|path| path.canonicalize().unwrap()),
            Some(sleeper.directory.path().canonicalize().unwrap())
        );
        assert_eq!(parent(pid), Some(std::process::id()));
        assert_eq!(name(pid).as_deref(), Some("sleep"));
        assert_eq!(
            arguments(pid),
            Some(vec!["/bin/sleep".to_owned(), "30".to_owned()])
        );
    }

    #[test]
    fn the_newest_child_is_the_one_started_last() {
        use std::os::unix::process::CommandExt;
        // A shell of the test's own, so other tests' children never count.
        let mut shell = Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 30 & /bin/sleep 0.2; /bin/sleep 31 & wait"])
            .process_group(0)
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let shell_pid = shell.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let newest = loop {
            let newest = newest_child(shell_pid).and_then(arguments);
            if newest
                .as_ref()
                .is_some_and(|arguments| arguments.last().map(String::as_str) == Some("31"))
                || std::time::Instant::now() >= deadline
            {
                break newest;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        // SAFETY: the group is the shell's own, led by the child started above.
        unsafe { libc::kill(-libc::pid_t::try_from(shell_pid).unwrap(), libc::SIGKILL) };
        let _ = shell.wait();
        assert_eq!(newest, Some(vec!["/bin/sleep".to_owned(), "31".to_owned()]));
    }

    #[test]
    fn an_ended_process_answers_nothing() {
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(working_directory(pid), None);
        assert_eq!(parent(pid), None);
        assert_eq!(name(pid), None);
        assert_eq!(arguments(pid), None);
    }
}
