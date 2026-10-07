use super::*;
use std::sync::{Arc, mpsc};
use std::time::Duration;

#[test]
fn unbuffered_negotiation_deadline_covers_trickled_header_and_payload() {
    use super::super::{ClientFrame, protocol, read_frame_with_timeout};
    for prefix in [1, 4] {
        let (input, mut source) = UnixStream::pair().unwrap();
        let mut input = std::fs::File::from(std::os::fd::OwnedFd::from(input));
        let mut wire = Vec::new();
        protocol::write_frame(
            &mut wire,
            &ClientFrame::Hello {
                version: protocol::PROTOCOL_VERSION,
                token: String::new(),
            },
        )
        .unwrap();
        source.write_all(&wire[..prefix]).unwrap();
        let sender = thread::spawn(move || {
            for byte in &wire[prefix..] {
                thread::sleep(Duration::from_millis(3));
                if source.write_all(&[*byte]).is_err() {
                    return;
                }
            }
        });
        let result = read_frame_with_timeout::<ClientFrame>(&mut input, Duration::from_millis(40));
        drop(input);
        sender.join().unwrap();
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
    }
}

#[test]
fn input_eof_half_closes_and_drains_final_daemon_response() {
    let (bridge, mut daemon) = UnixStream::pair().unwrap();
    daemon
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (input, mut source) = UnixStream::pair().unwrap();
    let hello = super::super::ClientFrame::Hello {
        version: super::super::protocol::PROTOCOL_VERSION,
        token: String::new(),
    };
    let mut coalesced = Vec::new();
    super::super::protocol::write_frame(&mut coalesced, &hello).unwrap();
    coalesced.extend_from_slice(b"request\0bytes");
    source.write_all(&coalesced).unwrap();
    source.shutdown(Shutdown::Write).unwrap();
    let mut input = std::fs::File::from(std::os::fd::OwnedFd::from(input));
    assert_eq!(
        super::super::protocol::read_frame::<super::super::ClientFrame>(&mut input).unwrap(),
        hello
    );
    let server = thread::spawn(move || {
        let mut request = Vec::new();
        daemon.read_to_end(&mut request).unwrap();
        assert_eq!(request, b"request\0bytes");
        daemon.write_all(b"final\0response").unwrap();
    });
    let mut output = Vec::new();
    let result = run(bridge, &mut output, || input);
    server.join().unwrap();
    result.unwrap();
    assert_eq!(output, b"final\0response");
}

#[test]
fn output_eof_failure_and_panic_join_idle_input_before_return() {
    struct Input {
        stream: UnixStream,
        dropped: Arc<AtomicBool>,
    }
    impl AsRawFd for Input {
        fn as_raw_fd(&self) -> std::os::fd::RawFd {
            self.stream.as_raw_fd()
        }
    }
    impl Read for Input {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.stream.read(bytes)
        }
    }
    impl Drop for Input {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::Release);
        }
    }
    struct Sink(u8);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0 == 1 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            assert_ne!(self.0, 2, "fixture output panic");
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            if self.0 == 3 {
                Err(std::io::ErrorKind::BrokenPipe.into())
            } else {
                Ok(())
            }
        }
    }
    for mode in 0..4 {
        let (bridge, mut daemon) = UnixStream::pair().unwrap();
        let (input, source) = UnixStream::pair().unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&dropped);
        let (tx, rx) = mpsc::channel();
        let server = thread::spawn(move || {
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            if mode != 0 {
                daemon.write_all(b"output").unwrap();
            }
        });
        let (done, result_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(bridge, &mut Sink(mode), move || {
                    let input = Input {
                        stream: input,
                        dropped: observed,
                    };
                    tx.send(()).unwrap();
                    input
                })
            }));
            let _ = done.send((result, dropped.load(Ordering::Acquire)));
        });
        let completed = result_rx.recv_timeout(Duration::from_secs(2));
        // Release a regressed input read before joining and asserting.
        let _ = source.shutdown(Shutdown::Both);
        worker.join().unwrap();
        server.join().unwrap();
        let (result, input_dropped_on_return) =
            completed.expect("bridge did not cancel idle input");
        assert!(input_dropped_on_return, "input worker outlived bridge");
        match mode {
            0 => result.unwrap().unwrap(),
            2 => assert!(result.is_err()),
            _ => assert!(result.unwrap().is_err()),
        }
    }
}

#[test]
fn input_write_failure_stops_the_other_direction() {
    let (bridge, daemon) = UnixStream::pair().unwrap();
    daemon.shutdown(Shutdown::Read).unwrap();
    let (input, mut source) = UnixStream::pair().unwrap();
    source.write_all(b"request").unwrap();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = tx.send(run(bridge, &mut Vec::new(), || input));
    });
    let result = rx.recv_timeout(Duration::from_secs(2));
    let _ = daemon.shutdown(Shutdown::Both);
    let _ = source.shutdown(Shutdown::Both);
    worker.join().unwrap();
    let error = result
        .expect("input write failure left bridge blocked")
        .unwrap_err();
    assert!(error.to_string().contains("forward SSH input"), "{error:#}");
}

#[test]
fn input_read_failure_and_panic_wake_output_and_are_reported() {
    struct BadInput {
        stream: UnixStream,
        panic: bool,
    }
    impl AsRawFd for BadInput {
        fn as_raw_fd(&self) -> std::os::fd::RawFd {
            self.stream.as_raw_fd()
        }
    }
    impl Read for BadInput {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            assert!(!self.panic, "fixture input panic");
            Err(std::io::ErrorKind::InvalidData.into())
        }
    }
    for panic in [false, true] {
        let (bridge, daemon) = UnixStream::pair().unwrap();
        let (input, mut source) = UnixStream::pair().unwrap();
        source.write_all(b"ready").unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = tx.send(run(bridge, &mut Vec::new(), || BadInput {
                stream: input,
                panic,
            }));
        });
        let result = rx.recv_timeout(Duration::from_secs(2));
        // Release even a regressed output reader before joining/asserting.
        daemon.shutdown(Shutdown::Both).unwrap();
        worker.join().unwrap();
        let error = result
            .expect("input failure left output blocked")
            .unwrap_err();
        if panic {
            assert!(error.to_string().contains("panicked"));
        } else {
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::InvalidData
            );
        }
    }
}
