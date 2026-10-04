//! One shared frame-write transaction for attached input and resize producers.
use super::{ClientFrame, DomainCloser, protocol};
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

struct FailedFrame<'a> {
    alive: &'a AtomicBool,
    closer: &'a DomainCloser,
    armed: bool,
}

impl Drop for FailedFrame<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.alive.store(false, Ordering::Release);
            self.closer.close();
        }
    }
}

/// Returns false when the relay has already stopped; never retries a frame.
pub(super) fn send(
    writer: &Mutex<impl Write>,
    alive: &AtomicBool,
    closer: &DomainCloser,
    frame: &ClientFrame,
) -> anyhow::Result<bool> {
    if !alive.load(Ordering::Acquire) {
        return Ok(false);
    }
    let mut writer = match writer.lock() {
        Ok(writer) => writer,
        Err(poison) => {
            // PoisonError owns the acquired lock. Close before dropping it.
            alive.store(false, Ordering::Release);
            closer.close();
            drop(poison);
            anyhow::bail!("domain relay writer mutex poisoned");
        }
    };
    if !alive.load(Ordering::Acquire) {
        return Ok(false);
    }
    // Declared after the lock guard: on panic it closes before unlocking, too.
    let mut failure = FailedFrame {
        alive,
        closer,
        armed: true,
    };
    match protocol::write_frame(&mut *writer, frame) {
        Ok(()) => {
            failure.armed = false;
            Ok(true)
        }
        Err(error) => {
            let was_alive = alive.swap(false, Ordering::AcqRel);
            drop(failure); // Finish transport cleanup while still holding writer.
            if was_alive { Err(error) } else { Ok(false) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{ErrorKind, Read};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    fn transport() -> (DomainCloser, UnixStream) {
        let (client, peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        (DomainCloser::Local(client), peer)
    }

    fn frames() -> [ClientFrame; 2] {
        [
            ClientFrame::Input {
                data: b"input".to_vec(),
            },
            ClientFrame::Resize {
                cols: 80,
                rows: 24,
                pixel_width: 640,
                pixel_height: 480,
            },
        ]
    }

    #[test]
    fn failed_frame_stops_both_producers_before_returning() {
        struct FailOnce {
            bytes: Vec<u8>,
            limit: usize,
            failed: bool,
        }
        impl Write for FailOnce {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if !self.failed && self.bytes.len() == self.limit {
                    self.failed = true;
                    return Err(ErrorKind::BrokenPipe.into());
                }
                let count = if self.failed {
                    bytes.len()
                } else {
                    bytes.len().min(self.limit - self.bytes.len())
                };
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                if self.limit == usize::MAX && !self.failed {
                    self.failed = true;
                    return Err(ErrorKind::BrokenPipe.into());
                }
                Ok(())
            }
        }
        // Both producer orders, partial header/body, and a failed final flush.
        for reverse in [false, true] {
            for limit in [0, 2, 6, usize::MAX] {
                let (closer, mut peer) = transport();
                let alive = AtomicBool::new(true);
                let writer = Mutex::new(FailOnce {
                    bytes: Vec::new(),
                    limit,
                    failed: false,
                });
                let mut frames = frames();
                if reverse {
                    frames.reverse();
                }
                assert!(send(&writer, &alive, &closer, &frames[0]).is_err());
                assert!(!alive.load(Ordering::Acquire));
                let before = writer.lock().unwrap().bytes.clone();
                assert!(!send(&writer, &alive, &closer, &frames[1]).unwrap());
                assert_eq!(writer.lock().unwrap().bytes, before);
                assert_eq!(peer.read(&mut [0]).unwrap(), 0);
            }
        }
    }

    #[test]
    fn successful_frames_keep_transport_alive_and_preserve_wire_bytes() {
        let (closer, mut peer) = transport();
        let alive = AtomicBool::new(true);
        let writer = Mutex::new(Vec::new());
        for frame in frames() {
            assert!(send(&writer, &alive, &closer, &frame).unwrap());
        }
        assert!(alive.load(Ordering::Acquire));
        assert!(matches!(
            peer.read(&mut [0]).unwrap_err().kind(),
            ErrorKind::WouldBlock | ErrorKind::TimedOut
        ));
        let bytes = writer.lock().unwrap().clone();
        let mut wire = bytes.as_slice();
        for frame in frames() {
            assert_eq!(
                protocol::read_frame::<ClientFrame>(&mut wire).unwrap(),
                frame
            );
        }
        assert!(wire.is_empty());
        alive.store(false, Ordering::Release);
        assert!(!send(&writer, &alive, &closer, &frames()[0]).unwrap());
        assert_eq!(*writer.lock().unwrap(), bytes);
    }

    #[test]
    fn cleanup_keeps_writer_locked_until_transport_close_finishes() {
        use super::super::SshChild;
        use std::sync::{Arc, TryLockError};
        use std::time::Instant;
        struct Failure(bool);
        impl Write for Failure {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                assert!(!self.0, "fixture in-write panic");
                Err(ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for panic in [false, true] {
            let alive = Arc::new(AtomicBool::new(true));
            let writer = Arc::new(Mutex::new(Failure(panic)));
            // No process is spawned. Hold the existing child-owner mutex to
            // pause transport close deterministically after liveness is cleared.
            let closer = Arc::new(DomainCloser::Ssh(SshChild(Mutex::new(None))));
            let DomainCloser::Ssh(child) = closer.as_ref() else {
                unreachable!()
            };
            let hold_close = child.0.lock().unwrap();
            let worker_alive = Arc::clone(&alive);
            let worker_writer = Arc::clone(&writer);
            let worker_closer = Arc::clone(&closer);
            let worker = std::thread::spawn(move || {
                std::panic::catch_unwind(|| {
                    send(&worker_writer, &worker_alive, &worker_closer, &frames()[0])
                })
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            while alive.load(Ordering::Acquire)
                && !worker.is_finished()
                && Instant::now() < deadline
            {
                std::thread::yield_now();
            }
            let stopped = !alive.load(Ordering::Acquire);
            let still_locked = matches!(writer.try_lock(), Err(TryLockError::WouldBlock));
            // Always release the fixture's transport lock before asserting.
            drop(hold_close);
            let result = worker.join().unwrap();
            assert!(
                stopped && still_locked,
                "writer unlocked before close: panic={panic}"
            );
            if panic {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_err());
            }
        }
    }

    #[test]
    fn writer_panic_and_existing_poison_stop_the_transport() {
        struct PanicWrite;
        impl Write for PanicWrite {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                panic!("fixture write panic")
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for pre_poisoned in [false, true] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let writer = Mutex::new(PanicWrite);
            if pre_poisoned {
                let _ = std::panic::catch_unwind(|| {
                    let _lock = writer.lock().unwrap();
                    panic!("fixture lock poison");
                });
            }
            let result = std::panic::catch_unwind(|| send(&writer, &alive, &closer, &frames()[0]));
            if pre_poisoned {
                assert!(result.unwrap().is_err());
            } else {
                assert!(result.is_err());
            }
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }
}
