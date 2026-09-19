//! Output-worker completion and transport ownership for one attached relay.
use super::{DomainCloser, ServerFrame, protocol};
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

pub(super) fn run(
    reader: &mut impl Read,
    output: &mut impl Write,
    alive: &AtomicBool,
    closer: &DomainCloser,
) -> anyhow::Result<()> {
    let _completion = Completion { alive, closer };
    loop {
        match protocol::read_frame::<ServerFrame>(reader) {
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
            Err(_) => break,
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
