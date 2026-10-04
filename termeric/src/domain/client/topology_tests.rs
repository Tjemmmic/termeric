//! Local socket fixtures for the post-authentication subscription lifecycle.
use super::super::model::DomainTopology;
use super::*;
use std::sync::mpsc;

fn topology() -> DomainTopology {
    DomainTopology {
        domain_id: "fixture".into(),
        active_tab: 1,
        tabs: Vec::new(),
    }
}

fn initial_frame() -> ServerFrame {
    ServerFrame::Response {
        response: Response::TopologySubscribed {
            revision: 1,
            domain: topology(),
        },
    }
}

fn update_frame() -> ServerFrame {
    ServerFrame::TopologyChanged {
        revision: 2,
        domain: topology(),
    }
}

struct Fixture {
    peer: UnixStream,
    observer: UnixStream,
    alive: Arc<AtomicBool>,
    events: mpsc::Receiver<DomainTopologyEvent>,
    result: mpsc::Receiver<anyhow::Result<()>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn new(timeout: Duration) -> Self {
        let (client, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let observer = client.try_clone().unwrap();
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = Arc::clone(&alive);
        let (event_tx, events) = mpsc::channel();
        let (result_tx, result) = mpsc::channel();
        let worker = thread::spawn(move || {
            let result = subscribe_topology_stream(
                DomainStream::Local(client),
                "fixture".into(),
                &worker_alive,
                timeout,
                |event| {
                    event_tx.send(event).unwrap();
                    true
                },
            );
            let _ = result_tx.send(result);
        });
        assert!(matches!(
            protocol::read_frame::<ClientFrame>(&mut peer).unwrap(),
            ClientFrame::Request {
                request: Request::SubscribeTopology { .. }
            }
        ));
        Self {
            peer,
            observer,
            alive,
            events,
            result,
            worker: Some(worker),
        }
    }

    fn initialize(&mut self) {
        protocol::write_frame(&mut self.peer, &initial_frame()).unwrap();
        assert!(matches!(
            self.events.recv_timeout(Duration::from_secs(2)).unwrap(),
            DomainTopologyEvent::Snapshot { revision: 1, .. }
        ));
    }

    fn wait_until_consumed(&self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let mut available: libc::c_int = 0;
            // SAFETY: FIONREAD writes one c_int and does not read/drain bytes;
            // this cloned descriptor remains live for the complete fixture.
            assert_eq!(
                unsafe { libc::ioctl(self.observer.as_raw_fd(), libc::FIONREAD, &mut available) },
                0
            );
            if available == 0 {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "reader never consumed partial bytes"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let _ = self.peer.shutdown(std::net::Shutdown::Both);
        let _ = self.observer.shutdown(std::net::Shutdown::Both);
        // Release even a regressed blocking reader before the test can finish.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn topology_cancellation_interrupts_partial_initial_and_update_frames() {
    for initial in [true, false] {
        for bytes in [1, 6] {
            let mut fixture = Fixture::new(Duration::from_secs(5));
            if !initial {
                fixture.initialize();
            }
            let mut frame = Vec::new();
            protocol::write_frame(
                &mut frame,
                &if initial {
                    initial_frame()
                } else {
                    update_frame()
                },
            )
            .unwrap();
            fixture.peer.write_all(&frame[..bytes]).unwrap();
            fixture.wait_until_consumed();
            fixture.alive.store(false, Ordering::Release);
            let result = fixture.result.recv_timeout(Duration::from_secs(2));
            let no_event = fixture.events.try_recv().is_err();
            drop(fixture);
            result
                .expect("cancelled partial topology read retained its worker")
                .expect("cancellation is a normal subscription exit");
            assert!(
                no_event,
                "cancelled topology delivered a partial/stale event"
            );
        }
    }
}

#[test]
fn topology_trickled_update_cannot_renew_the_frame_deadline() {
    for prefix in [1, 4] {
        let mut fixture = Fixture::new(Duration::from_millis(100));
        fixture.initialize();
        let mut frame = Vec::new();
        let mut domain = topology();
        domain.domain_id = "long fixture".repeat(100);
        protocol::write_frame(
            &mut frame,
            &ServerFrame::TopologyChanged {
                revision: 2,
                domain,
            },
        )
        .unwrap();
        fixture.peer.write_all(&frame[..prefix]).unwrap();
        let mut peer = fixture.peer.try_clone().unwrap();
        let alive = Arc::clone(&fixture.alive);
        let writer = thread::spawn(move || {
            let mut sent = 0;
            for byte in &frame[prefix..] {
                thread::sleep(Duration::from_millis(5));
                if !alive.load(Ordering::Acquire) || peer.write_all(&[*byte]).is_err() {
                    break;
                }
                sent += 1;
            }
            sent
        });
        let completed = fixture.result.recv_timeout(Duration::from_secs(2));
        drop(fixture);
        assert!(
            writer.join().unwrap() > 1,
            "fixture made no incremental progress"
        );
        let error = completed
            .expect("trickled frame retained its worker")
            .expect_err("incomplete frame passed");
        assert!(
            error
                .chain()
                .any(|cause| cause.to_string().contains("deadline elapsed")),
            "{error:#}"
        );
    }
}

#[test]
fn topology_idle_time_is_not_a_frame_deadline_and_revisions_stay_ordered() {
    let mut fixture = Fixture::new(Duration::from_millis(100));
    fixture.initialize();
    assert!(matches!(
        fixture.result.recv_timeout(Duration::from_millis(250)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    let mut wire = Vec::new();
    protocol::write_frame(&mut wire, &update_frame()).unwrap();
    for byte in wire {
        fixture.peer.write_all(&[byte]).unwrap();
    }
    assert!(matches!(
        fixture.events.recv_timeout(Duration::from_secs(2)).unwrap(),
        DomainTopologyEvent::Snapshot { revision: 2, .. }
    ));
    protocol::write_frame(
        &mut fixture.peer,
        &ServerFrame::DomainClosed {
            domain: "fixture".into(),
            message: "done".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        fixture.events.recv_timeout(Duration::from_secs(2)).unwrap(),
        DomainTopologyEvent::Closed { .. }
    ));
    fixture
        .result
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
}

#[test]
fn topology_precancelled_subscription_sends_nothing_and_callback_stop_detaches() {
    let (client, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    subscribe_topology_stream(
        DomainStream::Local(client),
        "fixture".into(),
        &AtomicBool::new(false),
        Duration::from_secs(5),
        |_| panic!("cancelled subscription delivered an event"),
    )
    .unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);

    let (client, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let worker = thread::spawn(move || {
        subscribe_topology_stream(
            DomainStream::Local(client),
            "fixture".into(),
            &AtomicBool::new(true),
            Duration::from_secs(2),
            |_| false,
        )
    });
    assert!(matches!(
        protocol::read_frame::<ClientFrame>(&mut peer).unwrap(),
        ClientFrame::Request {
            request: Request::SubscribeTopology { .. }
        }
    ));
    protocol::write_frame(&mut peer, &initial_frame()).unwrap();
    let detached = protocol::read_frame::<ClientFrame>(&mut peer);
    let _ = peer.shutdown(std::net::Shutdown::Both);
    worker.join().unwrap().unwrap();
    assert_eq!(detached.unwrap(), ClientFrame::Detach);
}
