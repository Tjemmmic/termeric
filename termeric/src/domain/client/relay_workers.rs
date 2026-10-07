//! Structured ownership of the threads started by an attached client relay.
use super::DomainCloser;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

/// Stops the relay and closes its transport when dropped, unless disarmed
/// first, so any early return, error, or unwind ends the other workers too.
pub(super) struct StopRelay<'a> {
    alive: &'a AtomicBool,
    closer: &'a DomainCloser,
    armed: bool,
}

impl<'a> StopRelay<'a> {
    pub(super) fn new(alive: &'a AtomicBool, closer: &'a DomainCloser) -> Self {
        Self {
            alive,
            closer,
            armed: true,
        }
    }

    /// Lets the relay go on after this guard drops.
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StopRelay<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.alive.store(false, Ordering::Release);
            // Never take the writer mutex: closing first wakes blocked writers.
            self.closer.close();
        }
    }
}

pub(super) struct Workers {
    alive: Arc<AtomicBool>,
    closer: Arc<DomainCloser>,
    pub(super) output: Option<JoinHandle<anyhow::Result<()>>>,
    pub(super) resize: Option<JoinHandle<anyhow::Result<()>>>,
}

impl Workers {
    pub(super) fn new(alive: Arc<AtomicBool>, closer: Arc<DomainCloser>) -> Self {
        Self {
            alive,
            closer,
            output: None,
            resize: None,
        }
    }

    pub(super) fn finish(&mut self) -> anyhow::Result<()> {
        self.alive.store(false, Ordering::Release);
        // Shutdown must precede any join. Never acquire the writer lock or
        // append Detach to a potentially blocked/partial outbound frame; EOF
        // releases this attachment through the daemon's existing exact-ID path.
        self.closer.close();
        let resize = self.resize.take().map(|worker| worker.join());
        let output = self.output.take().map(|worker| worker.join());
        // Join both workers before propagating either error.
        if let Some(output) = output {
            output.map_err(|_| anyhow::anyhow!("domain output relay panicked"))??;
        }
        if let Some(resize) = resize {
            resize.map_err(|_| anyhow::anyhow!("domain resize relay panicked"))??;
        }
        Ok(())
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        // Covers input/poll errors, a later worker failing to spawn, and
        // unwinding. On these paths the caller's original error stays primary.
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SshChild, relay_output};
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::{Mutex, mpsc};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn relay_worker_owner_cleans_partial_start_input_error_and_unwind() {
        for (resize, unwind) in [(false, false), (true, false), (true, true)] {
            let (client, mut peer) = UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut reader = client.try_clone().unwrap();
            let mut writer = client.try_clone().unwrap();
            crate::test_support::shrink_send_buffer(&client, 4096);
            let alive = Arc::new(AtomicBool::new(true));
            let closer = Arc::new(DomainCloser::Local(client));
            let (output_tx, output_done) = mpsc::channel();
            let (resize_tx, resize_done) = mpsc::channel();
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> anyhow::Result<()> {
                    let mut workers = Workers::new(Arc::clone(&alive), Arc::clone(&closer));
                    let output_alive = Arc::clone(&alive);
                    let output_closer = Arc::clone(&closer);
                    workers.output = Some(thread::spawn(move || {
                        let result = relay_output::run(
                            &mut reader,
                            &mut Vec::new(),
                            &output_alive,
                            &output_closer,
                        );
                        let _ = output_tx.send(());
                        result
                    }));
                    if resize {
                        workers.resize = Some(thread::spawn(move || {
                            let result = writer.write_all(&vec![0; 1024 * 1024]);
                            let _ = resize_tx.send(result);
                            Ok(())
                        }));
                        // Real send progress followed by a blocked large write;
                        // an unscheduled thread cannot satisfy the fixture.
                        peer.read_exact(&mut [0]).unwrap();
                        assert!(matches!(
                            resize_done.recv_timeout(Duration::from_millis(30)),
                            Err(mpsc::RecvTimeoutError::Timeout)
                        ));
                    }
                    assert!(!unwind, "fixture input panic");
                    anyhow::bail!("fixture input or later-spawn failure")
                }));
            let stopped_on_return = !alive.load(Ordering::Acquire);
            let output_joined = output_done.try_recv().is_ok();
            let resize_joined = !resize || resize_done.try_recv().is_ok();
            // Release even a regressed detached worker before asserting.
            alive.store(false, Ordering::Release);
            closer.close();
            let _ = peer.shutdown(std::net::Shutdown::Both);
            if !output_joined {
                output_done.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            if !resize_joined {
                let _ = resize_done.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            if unwind {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_err());
            }
            assert!(
                stopped_on_return && output_joined && resize_joined,
                "early exit left workers running: resize={resize}, unwind={unwind}"
            );
        }
    }

    #[test]
    fn relay_worker_finish_joins_every_handle_before_reporting_errors() {
        for failure in 0..5 {
            let (client, _peer) = UnixStream::pair().unwrap();
            let alive = Arc::new(AtomicBool::new(true));
            let mut workers =
                Workers::new(Arc::clone(&alive), Arc::new(DomainCloser::Local(client)));
            workers.output = Some(thread::spawn(move || {
                assert_ne!(failure, 1, "fixture output panic");
                if failure == 0 {
                    anyhow::bail!("fixture output failure");
                }
                Ok(())
            }));
            let resize_done = Arc::new(AtomicBool::new(false));
            let completed = Arc::clone(&resize_done);
            workers.resize = Some(thread::spawn(move || {
                while alive.load(Ordering::Acquire) {
                    thread::yield_now();
                }
                completed.store(true, Ordering::Release);
                assert_ne!(failure, 2, "fixture resize panic");
                if failure == 4 {
                    anyhow::bail!("fixture resize failure");
                }
                Ok(())
            }));
            let result = workers.finish();
            assert!(resize_done.load(Ordering::Acquire));
            if failure == 3 {
                result.unwrap();
            } else {
                let expected = match failure {
                    0 => "fixture output failure",
                    1 => "domain output relay panicked",
                    4 => "fixture resize failure",
                    _ => "domain resize relay panicked",
                };
                assert!(result.unwrap_err().to_string().contains(expected));
            }
            assert!(workers.output.is_none() && workers.resize.is_none());
            workers.finish().unwrap();
        }
    }

    #[test]
    fn relay_worker_owner_reaps_transport_if_no_worker_started() {
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        let alive = Arc::new(AtomicBool::new(true));
        let closer = Arc::new(DomainCloser::Ssh(SshChild(Mutex::new(Some(child)))));
        drop(Workers::new(Arc::clone(&alive), Arc::clone(&closer)));
        let stopped_on_return = !alive.load(Ordering::Acquire);
        // SAFETY: exact owned fixture child; this nonblocking wait sends no
        // signal and cannot reap another PID. No actual SSH client is invoked.
        let waited = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
        let error = std::io::Error::last_os_error();
        closer.close();
        assert!(stopped_on_return);
        assert_eq!(waited, -1);
        assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
    }
}
