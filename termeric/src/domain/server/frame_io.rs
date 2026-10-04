//! Bounded writes for complete server frames on local domain connections.
use super::protocol::{self, ServerFrame};
use crate::domain::io_util;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

pub(super) fn write_server_frame(
    stream: &mut UnixStream,
    frame: &ServerFrame,
) -> anyhow::Result<()> {
    let previous = stream.write_timeout()?;
    let result = protocol::write_frame(
        &mut FrameWriter {
            stream,
            deadline: Instant::now() + previous.unwrap_or(Duration::from_secs(5)),
        },
        frame,
    );
    if result.is_err() {
        // A partial header/body cannot be followed by another error response.
        // Interrupt readers too; their exact-owner cleanup remains idempotent.
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    result
}

struct FrameWriter<'a> {
    stream: &'a mut UnixStream,
    deadline: Instant,
}

const EXPIRED: &str = "domain server frame write deadline exceeded";

impl Write for FrameWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let stream = &*self.stream;
        io_util::write_before(self.deadline, stream.as_raw_fd(), EXPIRED, || {
            io_util::send_nonblocking(stream, bytes)
        })
    }

    fn flush(&mut self) -> std::io::Result<()> {
        io_util::remaining(self.deadline, EXPIRED)?;
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn slowly_drained_frame_cannot_renew_its_write_budget() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let capacity: libc::c_int = 4096;
        // SAFETY: the descriptor is live and the option points to a valid
        // c_int for exactly its byte size; this changes only this fixture.
        let result = unsafe {
            libc::setsockopt(
                server.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&capacity as *const libc::c_int).cast(),
                std::mem::size_of_val(&capacity) as libc::socklen_t,
            )
        };
        assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
        let timeout = Some(Duration::from_millis(250));
        server.set_write_timeout(timeout).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let reader = thread::spawn(move || {
            let mut total = 0;
            let mut buffer = [0; 1024];
            loop {
                match client.read(&mut buffer) {
                    Ok(0) => return total,
                    Ok(bytes) => total += bytes,
                    Err(error) => panic!("fixture reader failed: {error}"),
                }
                thread::sleep(Duration::from_millis(2));
            }
        });
        let frame = ServerFrame::SnapshotOutput {
            data: "x".repeat(1024 * 1024),
        };
        let result = write_server_frame(&mut server, &frame);
        assert_eq!(server.write_timeout().unwrap(), timeout);
        assert!(
            server
                .write_all(b"must-not-follow-a-partial-frame")
                .is_err()
        );
        let received = reader.join().unwrap();
        assert!(received > 0, "fixture must actually drain some output");
        assert!(
            received < 1024 * 1024,
            "expired writer transmitted the entire large frame: {received}"
        );
        let error = result.expect_err("partial writes renewed the complete-frame budget");
        let io = error.downcast_ref::<std::io::Error>().unwrap();
        assert!(matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
    }

    #[test]
    fn complete_frames_preserve_wire_bytes_and_restore_socket_policy() {
        for timeout in [None, Some(Duration::from_secs(3))] {
            let (mut server, mut client) = UnixStream::pair().unwrap();
            server.set_write_timeout(timeout).unwrap();
            let reader_clone = server.try_clone().unwrap();
            // SAFETY: F_GETFL only reads flags from the live fixture socket.
            let original_flags = unsafe { libc::fcntl(reader_clone.as_raw_fd(), libc::F_GETFL) };
            assert!(original_flags >= 0);
            client
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            for frame in [
                ServerFrame::Hello {
                    version: protocol::PROTOCOL_VERSION,
                },
                ServerFrame::SnapshotComplete,
            ] {
                let mut expected = Vec::new();
                protocol::write_frame(&mut expected, &frame).unwrap();
                write_server_frame(&mut server, &frame).unwrap();
                assert_eq!(server.write_timeout().unwrap(), timeout);
                // SAFETY: the same live descriptor is retained across the write.
                assert_eq!(
                    unsafe { libc::fcntl(reader_clone.as_raw_fd(), libc::F_GETFL) },
                    original_flags
                );
                let mut actual = vec![0; expected.len()];
                client.read_exact(&mut actual).unwrap();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn disconnected_peer_failure_restores_socket_policy() {
        let (mut server, client) = UnixStream::pair().unwrap();
        let previous = Some(Duration::from_secs(3));
        server.set_write_timeout(previous).unwrap();
        drop(client);
        assert!(write_server_frame(&mut server, &ServerFrame::SnapshotComplete).is_err());
        assert_eq!(server.write_timeout().unwrap(), previous);
    }

    #[test]
    fn expired_frame_writer_emits_no_partial_header() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let result = protocol::write_frame(
            &mut FrameWriter {
                stream: &mut server,
                deadline: Instant::now(),
            },
            &ServerFrame::SnapshotComplete,
        );
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        drop(server);
        assert_eq!(client.read(&mut [0; 1]).unwrap(), 0);
    }
}
