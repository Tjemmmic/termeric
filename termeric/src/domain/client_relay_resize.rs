//! Resize publication for one attached relay, sharing its transport lifetime.
use super::relay_workers::Completion;
use super::{ClientFrame, DomainCloser, relay_write};
use anyhow::Context;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use termeric_core::pty::PtySize;

pub(super) fn run(
    writer: &Mutex<impl Write>,
    alive: &AtomicBool,
    closer: &DomainCloser,
    mut size: impl FnMut() -> Option<PtySize>,
    mut wait: impl FnMut(),
) -> anyhow::Result<()> {
    let _completion = Completion { alive, closer };
    let mut previous = None;
    while alive.load(Ordering::Acquire) {
        if let Some(size) = size()
            && previous != Some(size)
        {
            if !relay_write::send(
                writer,
                alive,
                closer,
                &ClientFrame::Resize {
                    cols: size.cols,
                    rows: size.rows,
                    pixel_width: size.pixel_width,
                    pixel_height: size.pixel_height,
                },
            )
            .context("write attached terminal resize")?
            {
                break;
            }
            previous = Some(size);
        }
        wait();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::protocol;
    use std::cell::Cell;
    use std::io::{ErrorKind, Read};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    fn size(pixels: u16) -> PtySize {
        PtySize {
            cols: 80,
            rows: 24,
            pixel_width: pixels,
            pixel_height: 480,
        }
    }

    fn transport() -> (DomainCloser, UnixStream) {
        let (client, peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        (DomainCloser::Local(client), peer)
    }

    #[test]
    fn resize_failure_is_reported_without_retrying_a_partial_frame() {
        struct FailedWriter {
            remaining: usize,
        }
        impl Write for FailedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.remaining == 0 {
                    return Err(ErrorKind::BrokenPipe.into());
                }
                let count = self.remaining.min(bytes.len());
                self.remaining -= count;
                Ok(count)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(ErrorKind::BrokenPipe.into())
            }
        }
        // Fail before/within the header, within the payload, or at flush.
        for remaining in [0, 2, 6, usize::MAX] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let waits = Cell::new(0);
            let result = run(
                &Mutex::new(FailedWriter { remaining }),
                &alive,
                &closer,
                || Some(size(640)),
                || {
                    waits.set(waits.get() + 1);
                    alive.store(false, Ordering::Release);
                },
            );
            assert!(
                result.is_err(),
                "resize write failure became success: {remaining}"
            );
            assert_eq!(waits.get(), 0);
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn resize_preserves_geometry_and_deduplicates_only_identical_sizes() {
        let (closer, mut peer) = transport();
        let alive = AtomicBool::new(true);
        let writer = Mutex::new(Vec::new());
        let samples = [
            None,
            Some(size(640)),
            Some(size(640)),
            None,
            Some(size(800)),
        ];
        let index = Cell::new(0);
        run(
            &writer,
            &alive,
            &closer,
            || samples[index.get()],
            || {
                index.set(index.get() + 1);
                if index.get() == samples.len() {
                    alive.store(false, Ordering::Release);
                }
            },
        )
        .unwrap();
        let bytes = writer.into_inner().unwrap();
        let mut wire = bytes.as_slice();
        for pixels in [640, 800] {
            assert_eq!(
                protocol::read_frame::<ClientFrame>(&mut wire).unwrap(),
                ClientFrame::Resize {
                    cols: 80,
                    rows: 24,
                    pixel_width: pixels,
                    pixel_height: 480
                }
            );
        }
        assert!(wire.is_empty());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn resize_local_shutdown_skips_pending_writes_and_accepts_interrupted_writes() {
        struct ShutdownWriter<'a>(&'a AtomicBool, &'a Cell<usize>);
        impl Write for ShutdownWriter<'_> {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                self.1.set(self.1.get() + 1);
                self.0.store(false, Ordering::Release);
                Err(ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for before_write in [false, true] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let writes = Cell::new(0);
            let writer = Mutex::new(ShutdownWriter(&alive, &writes));
            run(
                &writer,
                &alive,
                &closer,
                || {
                    if before_write {
                        alive.store(false, Ordering::Release);
                    }
                    Some(size(640))
                },
                || panic!("stopped resize must not poll again"),
            )
            .unwrap();
            assert_eq!(writes.get(), usize::from(!before_write));
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn resize_poison_and_unwind_close_transport() {
        for poison in [false, true] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let writer = Mutex::new(Vec::new());
            if poison {
                let _ = std::panic::catch_unwind(|| {
                    let _lock = writer.lock().unwrap();
                    panic!("fixture poisoned writer");
                });
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(
                    &writer,
                    &alive,
                    &closer,
                    || {
                        assert!(poison, "fixture geometry panic");
                        Some(size(640))
                    },
                    || alive.store(false, Ordering::Release),
                )
            }));
            if poison {
                assert!(result.unwrap().is_err());
            } else {
                assert!(result.is_err());
            }
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }
}
