//! Socket-only coverage of the production live-frame deadline path.
use super::*;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, mpsc};
use std::thread;

const BUDGET: Duration = Duration::from_millis(60);

fn bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    protocol::write_frame(
        &mut bytes,
        &ServerFrame::Output {
            sequence: 1,
            data: vec![b'x'; 64],
        },
    )
    .unwrap();
    bytes
}

#[test]
fn live_trickled_header_and_body_cannot_renew_the_frame_deadline() {
    for prefix in [1, 4] {
        let (mut client, mut peer) = UnixStream::pair().unwrap();
        let closer = DomainCloser::Local(client.try_clone().unwrap());
        let alive = AtomicBool::new(true);
        let bytes = bytes();
        peer.write_all(&bytes[..prefix]).unwrap();
        let sender = thread::spawn(move || {
            for byte in &bytes[prefix..] {
                thread::sleep(Duration::from_millis(2));
                if peer.write_all(&[*byte]).is_err() {
                    return;
                }
            }
        });
        let mut output = Vec::new();
        let result = run_with_limits(&mut client, &mut output, &alive, &closer, BUDGET);
        drop(client);
        sender.join().unwrap();
        let error = result.expect_err("trickled live frame never expired");
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(output.is_empty());
        assert!(!alive.load(Ordering::Acquire));
    }
}

#[test]
fn idle_live_connections_outlast_frame_budgets_and_deliver_ordered_output() {
    let (mut client, mut peer) = UnixStream::pair().unwrap();
    let closer = DomainCloser::Local(client.try_clone().unwrap());
    let alive = AtomicBool::new(true);
    let sender = thread::spawn(move || {
        for sequence in [1, 2] {
            thread::sleep(BUDGET * 3);
            protocol::write_frame(
                &mut peer,
                &ServerFrame::Output {
                    sequence,
                    data: vec![b'0' + sequence as u8],
                },
            )
            .unwrap();
        }
        protocol::write_frame(
            &mut peer,
            &ServerFrame::Closed {
                exit_message: "done".into(),
            },
        )
        .unwrap();
    });
    let mut output = Vec::new();
    let result = run_with_limits(&mut client, &mut output, &alive, &closer, BUDGET);
    sender.join().unwrap();
    result.unwrap();
    assert_eq!(output, b"12\r\n[Termeric domain: done]\n");
}

#[test]
fn cancellation_releases_idle_and_partial_reads_without_transport_shutdown() {
    for prefix in [0, 1, 4] {
        let (mut client, mut peer) = UnixStream::pair().unwrap();
        let closer = DomainCloser::Local(client.try_clone().unwrap());
        let observer = client.try_clone().unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        peer.write_all(&bytes()[..prefix]).unwrap();
        let worker_alive = Arc::clone(&alive);
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut output = Vec::new();
            let result = run_with_limits(
                &mut client,
                &mut output,
                &worker_alive,
                &closer,
                Duration::from_secs(5),
            );
            let _ = tx.send((result, output));
        });
        // Ensure this isn't merely a completed read; the peer stays open and
        // contains no complete frame. Only the liveness flag is changed.
        assert!(matches!(
            rx.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        if prefix != 0
            && !crate::test_support::poll_until(Duration::from_secs(2), || {
                crate::test_support::unread_bytes(&observer) == 0
            })
        {
            let _ = peer.shutdown(std::net::Shutdown::Both);
            worker.join().unwrap();
            panic!("fixture did not consume the partial frame");
        }
        alive.store(false, Ordering::Release);
        let stopped = rx.recv_timeout(Duration::from_secs(2));
        // Release a regressed reader before asserting, without hanging tests.
        let _ = peer.shutdown(std::net::Shutdown::Both);
        worker.join().unwrap();
        let (result, output) = stopped.expect("cancellation did not release the live read");
        result.unwrap();
        assert!(output.is_empty());
    }
}

#[test]
fn stalled_live_header_and_payload_time_out_and_close_transport() {
    for prefix in [1, 4] {
        let (mut client, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let closer = DomainCloser::Local(client.try_clone().unwrap());
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = Arc::clone(&alive);
        peer.write_all(&bytes()[..prefix]).unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut output = Vec::new();
            let result = run_with_limits(&mut client, &mut output, &worker_alive, &closer, BUDGET);
            let _ = tx.send((result, output));
        });
        let stopped = rx.recv_timeout(Duration::from_secs(2));
        if stopped.is_err() {
            let _ = peer.shutdown(std::net::Shutdown::Both);
        }
        worker.join().unwrap();
        let (result, output) = stopped.expect("stalled live frame did not expire");
        assert_eq!(
            result
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .unwrap()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
        assert!(output.is_empty());
        assert!(!alive.load(Ordering::Acquire));
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }
}
