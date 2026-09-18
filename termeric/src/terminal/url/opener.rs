//! Bounded ownership of desktop opener children, independent of GUI lifetimes.

use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread::JoinHandle;

const MAX_OPENER_CHILDREN: usize = 8;
static SLOTS: OnceLock<Arc<AtomicUsize>> = OnceLock::new();

struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn acquire(slots: &Arc<AtomicUsize>) -> io::Result<Self> {
        slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_OPENER_CHILDREN).then_some(count + 1)
            })
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "all 8 desktop opener slots are busy; close an opened application and retry",
                )
            })?;
        Ok(Self(Arc::clone(slots)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn launch(command: Command) -> io::Result<()> {
    let slots = SLOTS.get_or_init(|| Arc::new(AtomicUsize::new(0)));
    // Dropping the thread handle detaches the waiter, not the child. The
    // worker continues through wait() even if a tab/window has disappeared.
    start(command, slots).map(drop)
}

fn start(command: Command, slots: &Arc<AtomicUsize>) -> io::Result<JoinHandle<()>> {
    let slot = Slot::acquire(slots)?;
    let (started, result) = mpsc::sync_channel(1);
    // Create the waiter before spawning its child. Thread creation failure
    // therefore cannot leave an unowned process behind. There is no queue of
    // launches, and both pending starts and running children consume a slot.
    let worker = std::thread::Builder::new()
        .name("termeric-opener".into())
        .spawn(move || run(command, slot, started))?;
    result
        .recv()
        .map_err(|_| io::Error::other("desktop opener worker stopped before launch"))??;
    Ok(worker)
}

fn run(mut command: Command, slot: Slot, started: mpsc::SyncSender<io::Result<()>>) {
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            drop(slot);
            let _ = started.send(Err(error));
            return;
        }
    };
    // This is spawn acknowledgement, not desktop-handler acceptance. Losing
    // the receiver must not skip reaping. No stdout/stderr capture or polling
    // is needed; only this bounded worker waits for the child's lifetime.
    let _ = started.send(Ok(()));
    match child.wait() {
        Ok(status) if !status.success() => {
            log::warn!("Desktop opener exited unsuccessfully: {status}");
        }
        Err(error) => log::warn!("Could not reap desktop opener: {error}"),
        Ok(_) => {}
    }
    drop(child);
    drop(slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservations_are_bounded_and_return_on_drop() {
        let slots = Arc::new(AtomicUsize::new(0));
        let permits: Vec<_> = (0..MAX_OPENER_CHILDREN)
            .map(|_| Slot::acquire(&slots).unwrap())
            .collect();
        assert_eq!(slots.load(Ordering::Acquire), MAX_OPENER_CHILDREN);
        assert_eq!(
            Slot::acquire(&slots).err().unwrap().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(permits);
        assert_eq!(slots.load(Ordering::Acquire), 0);
        drop(Slot::acquire(&slots).unwrap());
        assert_eq!(slots.load(Ordering::Acquire), 0);
    }

    #[test]
    fn failed_spawn_returns_original_error_and_releases_slot() {
        let slots = Arc::new(AtomicUsize::new(0));
        let directory = tempfile::tempdir().unwrap();
        let command = Command::new(directory.path().join("nonexistent-opener"));
        assert_eq!(
            start(command, &slots).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(slots.load(Ordering::Acquire), 0);
    }

    #[test]
    fn concurrent_admission_cannot_exceed_the_shared_budget() {
        let slots = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(std::sync::Barrier::new(17));
        let (sent, received) = mpsc::channel();
        let workers: Vec<_> = (0..16)
            .map(|_| {
                let slots = Arc::clone(&slots);
                let barrier = Arc::clone(&barrier);
                let sent = sent.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let slot = Slot::acquire(&slots);
                    sent.send(slot.is_ok()).unwrap();
                    barrier.wait();
                    drop(slot);
                })
            })
            .collect();
        barrier.wait();
        let admitted = (0..16).filter(|_| received.recv().unwrap()).count();
        let retained = slots.load(Ordering::Acquire);
        // Release/join before assertions so a failure cannot strand waiters.
        barrier.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(admitted, MAX_OPENER_CHILDREN);
        assert_eq!(retained, MAX_OPENER_CHILDREN);
        assert_eq!(slots.load(Ordering::Acquire), 0);
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;

        fn start_gated(command: Command, slots: &Arc<AtomicUsize>) -> JoinHandle<()> {
            let slots = Arc::clone(slots);
            let (sent, received) = mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let _ = sent.send(start(command, &slots));
            });
            // A regression that waits for exit before acknowledgement must
            // fail, not hang the test while the fixture waits for our EOF.
            received
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("spawn acknowledgement waited for child exit")
                .unwrap()
        }

        fn fixture(path: &std::path::Path, status: u8) -> (Command, UnixStream) {
            let (input, release) = UnixStream::pair().unwrap();
            let mut command = Command::new("/bin/sh");
            // Only this fixed fixture script runs: no desktop or provider.
            command
                .args([
                    "-c",
                    "printf '%s' \"$$\" > \"$1\"; read line; exit \"$2\"",
                    "fixture",
                ])
                .arg(path)
                .arg(status.to_string())
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
        fn running_children_hold_global_budget_and_exits_are_reaped() {
            let slots = Arc::new(AtomicUsize::new(0));
            let directory = tempfile::tempdir().unwrap();
            let mut workers = Vec::new();
            let mut releases = Vec::new();
            for index in 0..MAX_OPENER_CHILDREN {
                let path = directory.path().join(index.to_string());
                let (command, release) = fixture(&path, index as u8);
                // start returns while the child still awaits EOF, not exit.
                workers.push((start_gated(command, &slots), path));
                releases.push(release);
            }
            assert_eq!(slots.load(Ordering::Acquire), MAX_OPENER_CHILDREN);
            // WouldBlock, not NotFound: refusal precedes child launch.
            assert_eq!(
                start(Command::new(directory.path().join("absent")), &slots)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::WouldBlock
            );
            drop(releases);
            for (worker, path) in workers {
                worker.join().unwrap();
                assert_reaped(&path);
            }
            assert_eq!(slots.load(Ordering::Acquire), 0);
            // Reuse after both successful and nonzero child exits.
            let path = directory.path().join("reused");
            let (command, release) = fixture(&path, 0);
            let worker = start_gated(command, &slots);
            drop(release);
            worker.join().unwrap();
            assert_reaped(&path);
            assert_eq!(slots.load(Ordering::Acquire), 0);
        }

        #[test]
        fn dropped_acknowledgement_still_reaps_the_child() {
            let slots = Arc::new(AtomicUsize::new(0));
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("pid");
            let (command, release) = fixture(&path, 0);
            drop(release);
            let (started, result) = mpsc::sync_channel(1);
            drop(result);
            run(command, Slot::acquire(&slots).unwrap(), started);
            assert_reaped(&path);
            assert_eq!(slots.load(Ordering::Acquire), 0);
        }
    }
}
