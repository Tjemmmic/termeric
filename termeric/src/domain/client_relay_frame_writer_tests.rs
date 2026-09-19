use super::*;
use crate::domain::client::{ClientFrame, DomainCloser, protocol, relay_write};
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

const BUDGET: Duration = Duration::from_millis(60);

fn pipe() -> (DomainWriter, File) {
    let (read, write) = std::io::pipe().unwrap();
    let read = File::from(OwnedFd::from(read));
    let write = OwnedFd::from(write);
    (
        DomainWriter::Ssh(std::process::ChildStdin::from(write)),
        read,
    )
}

#[test]
fn stalled_and_slowly_drained_sockets_cannot_renew_frame_budget() {
    for slow in [false, true] {
        let (client, mut peer) = UnixStream::pair().unwrap();
        let capacity: libc::c_int = 4096;
        // SAFETY: live fixture socket and initialized c_int option; only this
        // test's send-buffer capacity changes.
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
        peer.set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let closer = DomainCloser::Local(client.try_clone().unwrap());
        let alive = AtomicBool::new(true);
        let drain = thread::spawn(move || {
            let mut progress = 0;
            for _ in 0..150 {
                if slow {
                    match peer.read(&mut [0; 64]) {
                        Ok(0) => break,
                        Ok(n) => progress += n,
                        Err(error)
                            if matches!(
                                error.kind(),
                                ErrorKind::WouldBlock | ErrorKind::TimedOut
                            ) => {}
                        Err(_) => break,
                    }
                }
                thread::sleep(Duration::from_millis(1));
            }
            progress
        });
        let writer = Mutex::new(Writer::with_timeout(DomainWriter::Local(client), BUDGET).unwrap());
        let error = relay_write::send(
            &writer,
            &alive,
            &closer,
            &ClientFrame::Input {
                data: vec![b'x'; 256 * 1024],
            },
        )
        .unwrap_err();
        let progress = drain.join().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            ErrorKind::TimedOut
        );
        assert!(!alive.load(Ordering::Acquire));
        if slow {
            assert!(progress > 0, "fixture never drained any bytes");
        }
    }
}

#[test]
fn owned_ssh_pipe_writes_time_out_without_changing_reader_flags() {
    let (inner, reader) = pipe();
    // SAFETY: read-only query on the owned fixture descriptor.
    let flags = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    let mut writer = Writer::with_timeout(inner, BUDGET).unwrap();
    let peer = thread::spawn(move || {
        // Even a regressed blocking writer is released when this reader drops.
        thread::sleep(BUDGET * 4);
        // SAFETY: read-only flags query on the still-owned fixture pipe.
        unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) }
    });
    let error = protocol::write_frame(
        &mut writer,
        &ClientFrame::Input {
            data: vec![b'x'; 256 * 1024],
        },
    )
    .unwrap_err();
    let after = peer.join().unwrap();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        ErrorKind::TimedOut
    );
    assert_eq!(after, flags);
}

#[test]
fn header_payload_and_flush_share_one_budget() {
    let (socket, _peer) = UnixStream::pair().unwrap();
    let mut writer = Writer::with_timeout(DomainWriter::Local(socket), BUDGET).unwrap();
    writer.write_all(&4u32.to_be_bytes()).unwrap();
    thread::sleep(BUDGET * 2);
    assert_eq!(
        writer.write_all(b"body").unwrap_err().kind(),
        ErrorKind::TimedOut
    );
    assert_eq!(writer.flush().unwrap_err().kind(), ErrorKind::TimedOut);
}

#[test]
fn successful_socket_and_pipe_frames_get_fresh_budgets_after_flush() {
    let (socket, socket_reader) = UnixStream::pair().unwrap();
    let observer = socket.try_clone().unwrap();
    let socket_flags = unsafe { libc::fcntl(observer.as_raw_fd(), libc::F_GETFL) };
    let (pipe, pipe_reader) = pipe();
    for (inner, mut reader) in [
        (
            DomainWriter::Local(socket),
            Box::new(socket_reader) as Box<dyn Read>,
        ),
        (pipe, Box::new(pipe_reader)),
    ] {
        let mut writer = Writer::with_timeout(inner, BUDGET).unwrap();
        for data in [b"first".to_vec(), b"second".to_vec()] {
            let frame = ClientFrame::Input { data };
            protocol::write_frame(&mut writer, &frame).unwrap();
            assert_eq!(
                protocol::read_frame::<ClientFrame>(&mut reader).unwrap(),
                frame
            );
            thread::sleep(BUDGET * 2);
        }
    }
    assert_eq!(
        unsafe { libc::fcntl(observer.as_raw_fd(), libc::F_GETFL) },
        socket_flags
    );
}
