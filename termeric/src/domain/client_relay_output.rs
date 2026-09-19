//! Output-worker completion and transport ownership for one attached relay.
use super::{DomainCloser, ServerFrame, protocol};
use anyhow::Context;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};

struct Completion<'a> {
    alive: &'a AtomicBool,
    closer: &'a DomainCloser,
}

impl Drop for Completion<'_> {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        // Do not acquire the shared writer mutex: its owner may be blocked
        // writing to the peer. Closing the transport wakes that write first.
        self.closer.close();
    }
}

// Keep the shared length/JSON validator and avoid buffering or prefetching live
// bytes. read_exact reports UnexpectedEof for both an empty stream and a cut
// frame; only an observed zero-byte read before any frame bytes is clean EOF.
struct FrameRead<'a, R> {
    reader: &'a mut R,
    started: bool,
    eof: bool,
}

impl<R: Read> Read for FrameRead<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(buffer)?;
        self.started |= count != 0;
        self.eof |= count == 0 && !buffer.is_empty();
        Ok(count)
    }
}

pub(super) fn run(
    reader: &mut impl Read,
    output: &mut impl Write,
    alive: &AtomicBool,
    closer: &DomainCloser,
) -> anyhow::Result<()> {
    let _completion = Completion { alive, closer };
    loop {
        let mut frame_reader = FrameRead {
            reader,
            started: false,
            eof: false,
        };
        match protocol::read_frame::<ServerFrame>(&mut frame_reader) {
            Ok(ServerFrame::Output { data, .. }) => {
                output.write_all(&data)?;
                output.flush()?;
            }
            Ok(ServerFrame::Closed { exit_message }) => {
                writeln!(output, "\r\n[Termeric domain: {exit_message}]")?;
                output.flush()?;
                break;
            }
            Ok(ServerFrame::Error { message }) => anyhow::bail!("{message}"),
            Ok(_) => anyhow::bail!("unexpected frame in attached domain stream"),
            Err(error) => {
                if frame_reader.eof && !frame_reader.started {
                    break;
                }
                // The worker owner clears liveness before shutting down the
                // transport. That local teardown can interrupt a partial read;
                // it is not peer corruption. Decode errors remain errors.
                if !alive.load(Ordering::Acquire)
                    && error.downcast_ref::<std::io::Error>().is_some()
                {
                    break;
                }
                return Err(error).context("read attached domain output");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    fn wire(frames: &[ServerFrame]) -> Cursor<Vec<u8>> {
        let mut bytes = Vec::new();
        for frame in frames {
            protocol::write_frame(&mut bytes, frame).unwrap();
        }
        Cursor::new(bytes)
    }

    fn transport() -> (DomainCloser, UnixStream) {
        let (client, peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        (DomainCloser::Local(client), peer)
    }

    #[test]
    fn relay_output_rejects_corrupt_or_truncated_frames_without_forwarding_them() {
        let valid = wire(&[ServerFrame::Output {
            sequence: 1,
            data: b"visible".to_vec(),
        }])
        .into_inner();
        let mut failures = vec![
            0u32.to_be_bytes().to_vec(),
            ((protocol::MAX_FRAME_BYTES + 1) as u32)
                .to_be_bytes()
                .to_vec(),
            [1u32.to_be_bytes().as_slice(), b"{"].concat(),
        ];
        // Every incomplete header and payload boundary, not just empty JSON.
        failures.extend((1..valid.len()).map(|end| valid[..end].to_vec()));
        for suffix in failures {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let mut output = Vec::new();
            let bytes = [valid.as_slice(), suffix.as_slice()].concat();
            let result = run(&mut bytes.as_slice(), &mut output, &alive, &closer);
            assert!(result.is_err(), "bad frame became success: {suffix:?}");
            assert_eq!(output, b"visible");
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn relay_output_transport_errors_are_not_clean_eof() {
        struct FailedRead(ErrorKind);
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(self.0.into())
            }
        }
        for kind in [
            ErrorKind::ConnectionReset,
            ErrorKind::UnexpectedEof,
            ErrorKind::TimedOut,
        ] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let error = run(&mut FailedRead(kind), &mut Vec::new(), &alive, &closer).unwrap_err();
            assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(), kind);
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn relay_output_decode_errors_are_not_hidden_by_local_shutdown() {
        let (closer, mut peer) = transport();
        let alive = AtomicBool::new(false);
        let bytes = [1u32.to_be_bytes().as_slice(), b"{"].concat();
        let error = run(&mut bytes.as_slice(), &mut Vec::new(), &alive, &closer).unwrap_err();
        assert!(error.downcast_ref::<serde_json::Error>().is_some());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn relay_output_fragmented_interrupted_frames_still_complete() {
        struct Fragmented {
            bytes: Cursor<Vec<u8>>,
            interrupt: bool,
        }
        impl Read for Fragmented {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.interrupt = !self.interrupt;
                if self.interrupt {
                    Err(ErrorKind::Interrupted.into())
                } else {
                    let count = buffer.len().min(1);
                    self.bytes.read(&mut buffer[..count])
                }
            }
        }
        let (closer, _peer) = transport();
        let alive = AtomicBool::new(true);
        let mut output = Vec::new();
        run(
            &mut Fragmented {
                bytes: wire(&[ServerFrame::Output {
                    sequence: 1,
                    data: b"exact".to_vec(),
                }]),
                interrupt: false,
            },
            &mut output,
            &alive,
            &closer,
        )
        .unwrap();
        assert_eq!(output, b"exact");
    }

    #[test]
    fn relay_output_local_shutdown_during_partial_frame_is_not_a_peer_failure() {
        struct CancelledRead<'a> {
            alive: &'a AtomicBool,
            first: bool,
        }
        impl Read for CancelledRead<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                if self.first {
                    self.first = false;
                    buffer[0] = 0;
                    return Ok(1);
                }
                self.alive.store(false, Ordering::Release);
                Ok(0)
            }
        }
        let (closer, mut peer) = transport();
        let alive = AtomicBool::new(true);
        let mut output = Vec::new();
        run(
            &mut CancelledRead {
                alive: &alive,
                first: true,
            },
            &mut output,
            &alive,
            &closer,
        )
        .unwrap();
        assert!(output.is_empty());
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn relay_output_error_frames_always_stop_siblings_and_close_transport() {
        for frame in [
            ServerFrame::Error {
                message: "fixture rejection".into(),
            },
            ServerFrame::SnapshotComplete,
        ] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let result = run(&mut wire(&[frame]), &mut Vec::new(), &alive, &closer);
            assert!(result.is_err());
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn relay_output_write_flush_and_panic_failures_always_complete() {
        struct Sink {
            fail: u8,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.fail == 0 {
                    return Err(ErrorKind::BrokenPipe.into());
                }
                assert_ne!(self.fail, 2, "fixture output panic");
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(ErrorKind::BrokenPipe.into())
            }
        }
        for fail in 0..3 {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(
                    &mut wire(&[ServerFrame::Output {
                        sequence: 1,
                        data: b"output".to_vec(),
                    }]),
                    &mut Sink { fail },
                    &alive,
                    &closer,
                )
            }));
            if fail == 2 {
                assert!(result.is_err());
            } else {
                assert!(result.unwrap().is_err());
            }
            assert!(!alive.load(Ordering::Acquire), "failure={fail}");
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn relay_output_eof_and_closed_preserve_output_and_close_once() {
        for closed in [false, true] {
            let (closer, mut peer) = transport();
            let alive = AtomicBool::new(true);
            let mut frames = vec![ServerFrame::Output {
                sequence: 1,
                data: b"hello".to_vec(),
            }];
            let mut expected = b"hello".to_vec();
            if closed {
                frames.push(ServerFrame::Closed {
                    exit_message: "done".into(),
                });
                expected.extend_from_slice(b"\r\n[Termeric domain: done]\n");
            }
            let mut output = Vec::new();
            run(&mut wire(&frames), &mut output, &alive, &closer).unwrap();
            assert_eq!(output, expected);
            assert!(!alive.load(Ordering::Acquire));
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
            closer.close();
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
        }
    }

    #[test]
    fn relay_output_failure_reaps_the_owned_transport_child() {
        use super::super::SshChild;
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        // A local inert child stands in for SSH; no SSH binary, host, or
        // authentication is involved. The existing owner handles cleanup.
        let closer = DomainCloser::Ssh(SshChild(std::sync::Mutex::new(Some(child))));
        let alive = AtomicBool::new(true);
        assert!(
            run(
                &mut wire(&[ServerFrame::Error {
                    message: "fixture failure".into()
                }]),
                &mut Vec::new(),
                &alive,
                &closer,
            )
            .is_err()
        );
        assert!(!alive.load(Ordering::Acquire));
        // SAFETY: this is the exact directly spawned fixture PID. WNOHANG
        // cannot wait for another process and does not send a signal.
        let result = unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) };
        assert_eq!(result, -1, "transport child was not reaped by its owner");
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        closer.close();
    }

    #[test]
    fn relay_output_error_releases_a_writer_blocked_on_the_same_connection() {
        let (client, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let capacity: libc::c_int = 4096;
        // SAFETY: live fixture socket; the option points to one initialized
        // c_int for its exact size and changes only this test's send buffer.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    client.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&capacity as *const libc::c_int).cast(),
                    std::mem::size_of_val(&capacity) as libc::socklen_t,
                )
            },
            0
        );
        let mut reader = client.try_clone().unwrap();
        let mut writer = client.try_clone().unwrap();
        let closer = Arc::new(DomainCloser::Local(client));
        let alive = Arc::new(AtomicBool::new(true));
        let (sent, received) = mpsc::channel();
        let input = thread::spawn(move || {
            let _ = sent.send(writer.write_all(&vec![0; 1024 * 1024]));
        });
        // Observe real send progress before asserting that the large write is
        // blocked; an unscheduled writer must not satisfy this fixture.
        peer.read_exact(&mut [0]).unwrap();
        let was_blocked = matches!(
            received.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        let output_closer = Arc::clone(&closer);
        let output_alive = Arc::clone(&alive);
        let (finished, completion) = mpsc::channel();
        let output = thread::spawn(move || {
            let _ = finished.send(run(
                &mut reader,
                &mut Vec::new(),
                &output_alive,
                &output_closer,
            ));
        });
        protocol::write_frame(
            &mut peer,
            &ServerFrame::Error {
                message: "stop fixture".into(),
            },
        )
        .unwrap();
        let output_result = completion.recv_timeout(Duration::from_secs(2));
        let input_result = received.recv_timeout(Duration::from_secs(2));
        // Ensure even a regression releases both workers before asserting.
        closer.close();
        let _ = peer.shutdown(std::net::Shutdown::Both);
        input.join().unwrap();
        output.join().unwrap();
        assert!(
            was_blocked,
            "fixture input was not blocked before output failed"
        );
        assert!(output_result.unwrap().is_err());
        assert!(
            input_result
                .expect("output failure left input blocked")
                .is_err()
        );
        assert!(!alive.load(Ordering::Acquire));
    }
}
