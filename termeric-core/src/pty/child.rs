//! Child status follows PTY ownership without retaining the terminal or output.

use portable_pty::{Child, ExitStatus};
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

struct State {
    child: Box<dyn Child + Send + Sync>,
    exit: Option<ExitStatus>,
    terminal_closed: bool,
}

impl State {
    fn poll(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.exit.is_none() {
            self.exit = self.child.try_wait()?;
        }
        Ok(self.exit.clone())
    }
}

pub(super) struct PtyChild(Arc<Mutex<State>>);

/// A stable observation of the initial PTY child. Cloning this handle does not
/// retain the PTY, its output, or its terminal; closing the PTY still terminates
/// its child normally. Observation never blocks waiting for child exit.
#[derive(Clone)]
pub struct ChildObserver(Arc<Mutex<State>>);

/// Portable child result. In portable-pty 0.8, signal termination is normalized
/// to code 1; this is not a POSIX wait status or a `128 + signal` convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildExitStatus {
    pub exit_code: u32,
    pub success: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildObservation {
    pub exit: Option<ChildExitStatus>,
    /// Whether the PTY control owner has been dropped. This is independent of
    /// natural child exit and does not imply output has been fully drained.
    pub terminal_closed: bool,
}

fn lock(state: &Mutex<State>) -> io::Result<MutexGuard<'_, State>> {
    state
        .lock()
        .map_err(|_| io::Error::other("PTY child state is unavailable"))
}

impl PtyChild {
    pub(super) fn new(child: Box<dyn Child + Send + Sync>) -> Self {
        Self(Arc::new(Mutex::new(State {
            child,
            exit: None,
            terminal_closed: false,
        })))
    }

    pub(super) fn observer(&self) -> ChildObserver {
        ChildObserver(Arc::clone(&self.0))
    }

    pub(super) fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        lock(&self.0)?.poll()
    }

    pub(super) fn wait(&self) -> io::Result<ExitStatus> {
        let mut state = lock(&self.0)?;
        if let Some(exit) = &state.exit {
            return Ok(exit.clone());
        }
        let exit = state.child.wait()?;
        state.exit = Some(exit.clone());
        Ok(exit)
    }

    pub(super) fn process_id(&self) -> Option<u32> {
        lock(&self.0).ok()?.child.process_id()
    }

    pub(super) fn close_terminal(&self) -> io::Result<()> {
        let mut state = lock(&self.0)?;
        state.terminal_closed = true;
        if state.poll().ok().flatten().is_none() {
            state.child.kill()?;
            // Keep a result reaped by the killer, or allow a surviving observer
            // to poll later if the platform's termination is asynchronous.
            let _ = state.poll();
        }
        Ok(())
    }
}

impl ChildObserver {
    pub fn poll(&self) -> io::Result<ChildObservation> {
        // A blocking wait on an unsplit Pty must not block an observer. Normal
        // control queries and owner polling use only try_wait.
        let mut state = self
            .0
            .try_lock()
            .map_err(|_| io::Error::other("PTY child state is busy or unavailable"))?;
        let exit = state.poll()?.map(|status| ChildExitStatus {
            exit_code: status.exit_code(),
            success: status.success(),
        });
        Ok(ChildObservation {
            exit,
            terminal_closed: state.terminal_closed,
        })
    }
}

impl Drop for PtyChild {
    fn drop(&mut self) {
        if let Err(error) = self.close_terminal() {
            log::warn!("Failed to terminate dropped PTY child: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::ChildKiller;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct OnceChild {
        polls: Arc<AtomicUsize>,
        fail: bool,
        signal: bool,
    }

    impl ChildKiller for OnceChild {
        fn kill(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(OnceChild {
                polls: Arc::clone(&self.polls),
                fail: self.fail,
                signal: self.signal,
            })
        }
    }
    impl Child for OnceChild {
        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            let count = self.polls.fetch_add(1, Ordering::Relaxed);
            if self.fail {
                return Err(io::Error::other("private child diagnostic"));
            }
            assert_eq!(count, 0, "already consumed child was polled again");
            Ok(Some(if self.signal {
                ExitStatus::with_signal("fixture-signal")
            } else {
                ExitStatus::with_exit_code(37)
            }))
        }
        fn wait(&mut self) -> io::Result<ExitStatus> {
            self.try_wait().map(Option::unwrap)
        }
        fn process_id(&self) -> Option<u32> {
            Some(1)
        }
        #[cfg(windows)]
        fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
            None
        }
    }

    #[test]
    fn observation_caches_consumed_status_and_preserves_portable_signal_semantics() {
        for signal in [false, true] {
            let polls = Arc::new(AtomicUsize::new(0));
            let child = PtyChild::new(Box::new(OnceChild {
                polls: polls.clone(),
                fail: false,
                signal,
            }));
            let observer = child.observer();
            let expected = Some(ChildExitStatus {
                exit_code: if signal { 1 } else { 37 },
                success: false,
            });
            assert_eq!(observer.poll().unwrap().exit, expected);
            assert!(!child.wait().unwrap().success());
            child.close_terminal().unwrap();
            drop(child);
            assert_eq!(
                observer.poll().unwrap(),
                ChildObservation {
                    exit: expected,
                    terminal_closed: true
                }
            );
            assert_eq!(polls.load(Ordering::Relaxed), 1);
        }
    }

    #[test]
    fn polling_failures_and_busy_state_are_not_running_or_success() {
        let child = PtyChild::new(Box::new(OnceChild {
            polls: Arc::new(AtomicUsize::new(0)),
            fail: true,
            signal: false,
        }));
        let observer = child.observer();
        assert!(observer.poll().is_err());
        let _guard = lock(&child.0).unwrap();
        assert!(observer.poll().is_err());
    }
}
