//! Starting a child that leads its own process group, signalling it, and
//! reaping it.
//!
//! Process-group signals are safe only while the group's leader is
//! unreaped: until then its id cannot be reused, so a signal to the group
//! cannot reach an unrelated one. A child started with [`spawn_piped`] is
//! therefore reaped only through [`reap`] and its group signalled only
//! through [`signal`], both by the one owner of its `Child`, never by
//! `Child::try_wait` or `Child::wait` while its group may still be
//! signalled.

use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

/// Where a group's leader is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leader {
    Running,
    /// Exited but unreaped, still holding its id.
    Exited,
    Reaped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Terminate,
    Kill,
}

/// A child leading its own process group, with its three pipes.
pub struct PipedChild {
    pub child: Child,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/// Starts `command` as the leader of a new process group, with stdin,
/// stdout, and stderr piped to Termeric.
pub fn spawn_piped(command: &mut Command) -> std::io::Result<PipedChild> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        signal(&child, Signal::Kill);
        let _ = child.wait();
        return Err(std::io::Error::other("the child's pipes were unavailable"));
    };
    Ok(PipedChild {
        child,
        stdin,
        stdout,
        stderr,
    })
}

/// Signals `leader` and everything in its group, unless it was reaped.
pub fn signal(leader: &Child, signal: Signal) {
    if leader_state(leader) != Leader::Reaped {
        signal_group(leader.id(), signal);
    }
}

/// Reaps `leader` once it has exited, first sending `survivors` to the
/// commands it left running in its group. `Ok(None)` while it runs.
pub fn reap(leader: &mut Child, survivors: Signal) -> std::io::Result<Option<ExitStatus>> {
    match leader_state(leader) {
        Leader::Running => Ok(None),
        Leader::Exited => {
            signal_group(leader.id(), survivors);
            leader.wait().map(Some)
        }
        // Reaped already: the status std kept.
        Leader::Reaped => leader.try_wait(),
    }
}

/// Where `leader` is in its life, without reaping it.
pub fn leader_state(leader: &Child) -> Leader {
    let id = libc::id_t::from(leader.id());
    loop {
        // SAFETY: a zeroed siginfo_t is a valid value, and waitid only
        // writes into it. WNOWAIT leaves an exited child unreaped.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                id,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            // With WNOHANG, nothing is written while the child runs.
            return if info.si_signo == libc::SIGCHLD {
                Leader::Exited
            } else {
                Leader::Running
            };
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            // ECHILD: no such unreaped child.
            return Leader::Reaped;
        }
    }
}

fn signal_group(process_id: u32, signal: Signal) {
    let Ok(group) = libc::pid_t::try_from(process_id) else {
        return;
    };
    if group <= 1 {
        return;
    }
    let signal = match signal {
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // The child leads its own process group; a negative id signals the
    // whole group, including the commands it started.
    unsafe {
        libc::kill(-group, signal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{poll_until, process_ends, process_is_alive};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    /// A group leader that starts a command in its group, prints that
    /// command's process id, and exits.
    fn leader_leaving_a_command() -> (Child, libc::pid_t) {
        let mut leader = Command::new("/bin/sh")
            .args(["-c", "sleep 30 </dev/null >/dev/null 2>&1 & echo $!"])
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut output = String::new();
        std::io::Read::read_to_string(&mut leader.stdout.take().unwrap(), &mut output).unwrap();
        let left = output.trim().parse().unwrap();
        assert!(
            poll_until(Duration::from_secs(5), || leader_state(&leader)
                != Leader::Running),
            "the leader never exited"
        );
        (leader, left)
    }

    #[test]
    fn reaping_an_exited_leader_first_ends_what_it_left_running() {
        let (mut leader, left) = leader_leaving_a_command();
        assert_eq!(leader_state(&leader), Leader::Exited);
        assert!(process_is_alive(left));
        assert!(reap(&mut leader, Signal::Kill).unwrap().unwrap().success());
        assert_eq!(leader_state(&leader), Leader::Reaped);
        assert!(process_ends(left));
        // The kept status answers again.
        assert!(reap(&mut leader, Signal::Kill).unwrap().is_some());
    }

    #[test]
    fn a_reaped_leaders_group_is_never_signalled() {
        let (mut leader, left) = leader_leaving_a_command();
        // Reaped behind this module's back, the group's id may be reused.
        leader.try_wait().unwrap().unwrap();
        signal(&leader, Signal::Kill);
        assert!(
            process_is_alive(left),
            "a reaped leader's group was signalled"
        );
        unsafe {
            libc::kill(left, libc::SIGKILL);
        }
    }

    #[test]
    fn a_running_leader_is_signalled_with_its_group() {
        let mut leader = Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .process_group(0)
            .spawn()
            .unwrap();
        assert_eq!(leader_state(&leader), Leader::Running);
        assert_eq!(reap(&mut leader, Signal::Kill).unwrap(), None);
        signal(&leader, Signal::Kill);
        assert!(!leader.wait().unwrap().success());
    }
}
