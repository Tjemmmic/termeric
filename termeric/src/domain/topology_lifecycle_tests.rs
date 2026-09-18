//! Socket-only lifecycle fixtures: no PTY, shell, desktop, or remote host.
use super::*;

const DOMAIN: &str = "11111111111111111111111111111111";

fn state() -> SharedState {
    let domain = DomainRuntime {
        id: DOMAIN.into(),
        name: "topology-fixture".into(),
        created_at: String::new(),
        workspace_id: DOMAIN.into(),
        active_tab: 0,
        next_tab: 0,
        next_pane: 0,
        next_split: 0,
        topology_revision: 1,
        topology_subscribers: Vec::new(),
        tabs: Vec::new(),
    };
    Arc::new(Mutex::new(ServerState {
        domains: HashMap::from([(DOMAIN.into(), domain)]),
        allow_screen_checksum_reports: false,
        xterm_keyboard_type: XtermKeyboardType::Normal,
        shutdown: Arc::new(AtomicBool::new(false)),
        wake_socket: None,
        next_attachment: AtomicU64::new(1),
        next_topology_subscription: AtomicU64::new(1),
        active_uploads: 0,
        upload_ids_in_flight: HashMap::new(),
        upload_records: HashMap::new(),
        upload_janitor_running: false,
    }))
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
    server
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
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
