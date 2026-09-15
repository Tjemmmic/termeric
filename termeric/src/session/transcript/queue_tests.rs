use super::*;

#[test]
fn failed_writer_releases_unread_queue_reservations_before_reporting_completion() {
    let (release_tx, release_rx) = mpsc::channel();
    let mut recorder = TranscriptRecorder::with_factory(
        PathBuf::from("failed-queue-fixture.jsonl"),
        metadata_entry(None, None, "synthetic", "default"),
        RecorderLimits {
            max_queue_bytes: 512,
            queue_entries: 4,
            ..RecorderLimits::default()
        },
        Box::new(move || {
            release_rx.recv().unwrap();
            Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "synthetic open failure",
            ))
        }),
        None,
    )
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            recorder.record_output("x".repeat(128)).unwrap(),
            TranscriptWrite::Written
        );
    }
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 512);
    release_tx.send(()).unwrap();
    recorder.worker.take().unwrap().join().unwrap();
    let retained_bytes = recorder.queued_bytes_for_diagnostics();
    // Do not consume the failure notice before testing the producer path:
    // stale byte charges must not mask disconnection as backpressure.
    let late_write = recorder.record_output("late output");
    let shutdown = recorder.finish(Duration::ZERO);
    assert_eq!(retained_bytes, 0, "unread entries left phantom queue bytes");
    assert_eq!(
        late_write.unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert_eq!(
        shutdown.unwrap_err().kind(),
        std::io::ErrorKind::StorageFull
    );
}

struct DiscardSink;

impl Write for DiscardSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TranscriptSink for DiscardSink {}

#[test]
fn rejected_sends_release_only_their_own_queue_charge() {
    let (release_tx, release_rx) = mpsc::channel();
    let mut recorder = TranscriptRecorder::with_factory(
        PathBuf::from("full-queue-ledger-fixture.jsonl"),
        metadata_entry(None, None, "synthetic", "default"),
        RecorderLimits {
            queue_entries: 1,
            max_queue_bytes: 512,
            ..RecorderLimits::default()
        },
        Box::new(move || {
            release_rx.recv().unwrap();
            Ok(Box::new(DiscardSink))
        }),
        None,
    )
    .unwrap();
    assert_eq!(
        recorder.record_output("x".repeat(128)).unwrap(),
        TranscriptWrite::Written
    );
    // This entry obtains byte capacity but is refused by the single-slot queue.
    assert_eq!(
        recorder.record_output("y".repeat(128)).unwrap(),
        TranscriptWrite::Backpressure
    );
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 256);
    // This one is refused before allocating an owned queue entry.
    assert_eq!(
        recorder.record_output("z".repeat(256)).unwrap(),
        TranscriptWrite::DroppedAtBackpressure
    );
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 256);
    release_tx.send(()).unwrap();
    recorder.finish(Duration::from_secs(2)).unwrap();
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 0);
    assert_eq!(
        recorder.record_output("after shutdown").unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 0);
}

#[test]
fn panicked_writer_releases_all_unread_reservations() {
    let (release_tx, release_rx) = mpsc::channel();
    let mut recorder = TranscriptRecorder::with_factory(
        PathBuf::from("panicked-queue-fixture.jsonl"),
        metadata_entry(None, None, "synthetic", "default"),
        RecorderLimits::default(),
        Box::new(move || {
            release_rx.recv().unwrap();
            panic!("synthetic factory panic with queued entries");
        }),
        None,
    )
    .unwrap();
    for _ in 0..8 {
        recorder.record_output("queued before panic").unwrap();
    }
    assert!(recorder.queued_bytes_for_diagnostics() > 0);
    release_tx.send(()).unwrap();
    assert!(recorder.worker.take().unwrap().join().is_err());
    assert_eq!(recorder.queued_bytes_for_diagnostics(), 0);
    assert_eq!(
        recorder.finish(Duration::ZERO).unwrap_err().kind(),
        std::io::ErrorKind::BrokenPipe
    );
}

#[test]
fn receiver_cleanup_preserves_a_live_producer_reservation() {
    let counter = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::sync_channel(2);
    let producer = QueueReservation::reserve(&counter, 200, 512).unwrap();
    assert!(
        sender
            .send(QueueReservation::reserve(&counter, 100, 512).unwrap())
            .is_ok()
    );
    assert_eq!(counter.load(Ordering::Acquire), 300);
    std::thread::spawn(move || drop(receiver)).join().unwrap();
    assert_eq!(counter.load(Ordering::Acquire), 200);
    assert!(QueueReservation::reserve(&counter, 313, 512).is_none());
    assert_eq!(counter.load(Ordering::Acquire), 200);
    // The disconnected channel returns the exact still-owned reservation.
    let Err(mpsc::SendError(returned)) = sender.send(producer) else {
        panic!("receiver was already destroyed");
    };
    assert_eq!(counter.load(Ordering::Acquire), 200);
    drop(returned);
    assert_eq!(counter.load(Ordering::Acquire), 0);
}

#[test]
fn concurrent_reservation_churn_never_exceeds_capacity_or_leaves_a_charge() {
    let counter = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let counter = Arc::clone(&counter);
            std::thread::spawn(move || {
                for _ in 0..1000 {
                    if let Some(reservation) = QueueReservation::reserve(&counter, 16, 64) {
                        assert!(counter.load(Ordering::Acquire) <= 64);
                        std::thread::yield_now();
                        drop(reservation);
                    }
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(counter.load(Ordering::Acquire), 0);
}
