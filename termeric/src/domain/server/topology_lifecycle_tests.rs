//! Socket-only lifecycle fixtures: no PTY, shell, desktop, or remote host.
use super::*;

use super::test_support::{DOMAIN, state};

#[test]
fn authenticated_client_must_send_a_complete_initial_request_before_setup_deadline() {
    let state = state();
    let clients = local_socket::ClientSlots::new(1);
    let slot = clients.claim().unwrap();
    let (server, mut client) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let _slot = slot;
        let result = handle_client_with_setup_timeout(
            server,
            "fixture-token",
            state,
            Duration::from_millis(40),
        );
        let _ = sender.send(result);
    });
    protocol::write_frame(
        &mut client,
        &ClientFrame::Hello {
            version: protocol::PROTOCOL_VERSION,
            token: "fixture-token".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
        ServerFrame::Hello { .. }
    ));
    let completed = receiver.recv_timeout(Duration::from_secs(2));
    // Even a regressed server must be released/joined before the assertion.
    client.shutdown(std::net::Shutdown::Both).unwrap();
    worker.join().unwrap();
    assert_eq!(clients.taken(), 0);
    let error = completed
        .expect("authenticated client retained its slot without a request")
        .expect_err("missing initial request must fail");
    let io = error.downcast_ref::<std::io::Error>().unwrap();
    assert!(matches!(
        io.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn partial_setup_header_and_payload_cannot_renew_the_deadline() {
    let frame = ClientFrame::Hello {
        version: protocol::PROTOCOL_VERSION,
        token: "fixture-token".repeat(8),
    };
    let mut encoded = Vec::new();
    protocol::write_frame(&mut encoded, &frame).unwrap();
    for initial_bytes in [1, 4] {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        client.write_all(&encoded[..initial_bytes]).unwrap();
        let remaining = encoded[initial_bytes..].to_vec();
        let writer = thread::spawn(move || {
            for byte in remaining {
                thread::sleep(Duration::from_millis(10));
                if client.write_all(&[byte]).is_err() {
                    break;
                }
            }
        });
        let result = read_client_frame_with_timeout(&mut server, Duration::from_millis(40));
        assert_eq!(server.read_timeout().unwrap(), None);
        drop(server);
        writer.join().unwrap();
        let error = result.expect_err("trickled setup frame renewed the read deadline");
        let io = error.downcast_ref::<std::io::Error>().unwrap();
        assert!(matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
    }
}

#[test]
fn complete_setup_frame_leaves_the_socket_timeout_alone() {
    let (mut server, mut client) = UnixStream::pair().unwrap();
    let previous = Some(Duration::from_secs(3));
    server.set_read_timeout(previous).unwrap();
    let frame = ClientFrame::Request {
        request: Request::ListDomains,
    };
    protocol::write_frame(&mut client, &frame).unwrap();
    assert_eq!(
        read_client_frame_with_timeout(&mut server, Duration::from_secs(1)).unwrap(),
        frame
    );
    assert_eq!(server.read_timeout().unwrap(), previous);
}

#[test]
fn established_subscription_does_not_inherit_the_setup_idle_timeout() {
    let state = state();
    let (server, mut client) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    protocol::write_frame(
        &mut client,
        &ClientFrame::Hello {
            version: protocol::PROTOCOL_VERSION,
            token: "fixture-token".into(),
        },
    )
    .unwrap();
    protocol::write_frame(
        &mut client,
        &ClientFrame::Request {
            request: Request::SubscribeTopology {
                domain: DOMAIN.into(),
            },
        },
    )
    .unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let _ = sender.send(handle_client_with_setup_timeout(
            server,
            "fixture-token",
            state,
            Duration::from_millis(40),
        ));
    });
    assert!(matches!(
        protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
        ServerFrame::Hello { .. }
    ));
    assert!(matches!(
        protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
        ServerFrame::Response {
            response: Response::TopologySubscribed { .. }
        }
    ));
    let early_completion = receiver.recv_timeout(Duration::from_millis(160));
    let _ = protocol::write_frame(&mut client, &ClientFrame::Detach);
    let final_completion = if early_completion.is_err() {
        Some(receiver.recv_timeout(Duration::from_secs(2)))
    } else {
        None
    };
    client.shutdown(std::net::Shutdown::Both).unwrap();
    worker.join().unwrap();
    assert!(
        matches!(
            early_completion,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "established subscription inherited the setup deadline"
    );
    final_completion.unwrap().unwrap().unwrap();
}

#[test]
fn failed_topology_initial_snapshot_releases_registration_without_a_later_publish() {
    let state = state();
    for _ in 0..MAX_TOPOLOGY_SUBSCRIBERS_PER_DOMAIN * 2 {
        let (server, client) = UnixStream::pair().unwrap();
        client.shutdown(std::net::Shutdown::Both).unwrap();
        drop(client);
        let error =
            serve_topology_subscription(server, Arc::clone(&state), DOMAIN.into()).unwrap_err();
        assert!(!error.to_string().contains("subscriber limit"));
        assert!(
            state.lock().unwrap().domains[DOMAIN]
                .topology_subscribers
                .is_empty(),
            "failed setup retained a subscriber slot"
        );
    }
}

#[test]
fn topology_queue_failure_disconnects_without_waiting_for_queued_frames() {
    for failure in ["overflow", "disconnected", "close-full"] {
        let state = state();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let _receiver = (failure != "disconnected").then_some(receiver);
        let (transport, mut client) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        {
            let mut guard = state.lock().unwrap();
            let domain = guard.domains.get_mut(DOMAIN).unwrap();
            domain.topology_subscribers.push(TopologySubscriber {
                id: 1,
                sender,
                transport,
            });
            domain.publish_topology();
            if failure == "close-full" {
                domain.close_topology_subscribers("fixture close");
            } else if failure == "overflow" {
                domain.publish_topology();
            }
            assert!(domain.topology_subscribers.is_empty());
        }
        assert_eq!(client.read(&mut [0; 1]).unwrap(), 0, "{failure}");
    }
}

fn subscribe(
    state: &SharedState,
) -> (
    UnixStream,
    std::sync::mpsc::Receiver<anyhow::Result<()>>,
    thread::JoinHandle<()>,
) {
    let (server, mut client) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let state = Arc::clone(state);
    let worker = thread::spawn(move || {
        let _ = sender.send(serve_topology_subscription(server, state, DOMAIN.into()));
    });
    assert!(matches!(
        protocol::read_frame::<ServerFrame>(&mut client).unwrap(),
        ServerFrame::Response {
            response: Response::TopologySubscribed { revision: 1, .. }
        }
    ));
    (client, receiver, worker)
}

#[test]
fn topology_detach_is_exact_and_healthy_subscribers_receive_ordered_close() {
    let state = state();
    let (mut first, first_done, first_worker) = subscribe(&state);
    let (mut second, second_done, second_worker) = subscribe(&state);
    protocol::write_frame(&mut first, &ClientFrame::Detach).unwrap();
    first_done
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    first_worker.join().unwrap();
    assert_eq!(first.read(&mut [0; 1]).unwrap(), 0);
    {
        let mut guard = state.lock().unwrap();
        let domain = guard.domains.get_mut(DOMAIN).unwrap();
        assert_eq!(domain.topology_subscribers.len(), 1);
        assert_eq!(domain.topology_subscribers[0].id, 2);
        domain.publish_topology();
    }
    assert!(matches!(
        protocol::read_frame::<ServerFrame>(&mut second).unwrap(),
        ServerFrame::TopologyChanged { revision: 2, .. }
    ));
    state
        .lock()
        .unwrap()
        .domains
        .get_mut(DOMAIN)
        .unwrap()
        .close_topology_subscribers("fixture closed");
    assert!(
        matches!(protocol::read_frame::<ServerFrame>(&mut second).unwrap(),
        ServerFrame::DomainClosed { domain, message }
            if domain == DOMAIN && message == "fixture closed")
    );
    second_done
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    second_worker.join().unwrap();
    assert_eq!(second.read(&mut [0; 1]).unwrap(), 0);
    assert!(
        state.lock().unwrap().domains[DOMAIN]
            .topology_subscribers
            .is_empty()
    );
}
