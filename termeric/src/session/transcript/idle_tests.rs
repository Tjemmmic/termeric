use super::*;
use std::sync::Mutex;

struct ObservedSink {
    waits: Arc<AtomicUsize>,
    zero_waits: Arc<AtomicUsize>,
    notify: SyncSender<()>,
}

impl Write for ObservedSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TranscriptSink for ObservedSink {
    fn before_receive(&mut self, timeout: Option<Duration>) {
        let count = self.waits.fetch_add(1, Ordering::SeqCst) + 1;
        if timeout == Some(Duration::ZERO) {
            self.zero_waits.fetch_add(1, Ordering::SeqCst);
        }
        if count == 1 || count == 1000 {
            let _ = self.notify.try_send(());
        }
    }
}

#[test]
fn an_idle_transcript_blocks_instead_of_polling_an_expired_flush_deadline() {
    let waits = Arc::new(AtomicUsize::new(0));
    let zero_waits = Arc::new(AtomicUsize::new(0));
    let (notify, observed) = mpsc::sync_channel(2);
    let sink = ObservedSink {
        waits: Arc::clone(&waits),
        zero_waits: Arc::clone(&zero_waits),
        notify,
    };
    let mut recorder = TranscriptRecorder::with_factory(
        PathBuf::from("idle-writer-fixture.jsonl"),
        metadata_entry(None, None, "synthetic", "default"),
        RecorderLimits::default(),
        Box::new(move || Ok(Box::new(sink))),
        None,
    )
    .unwrap();
    observed.recv_timeout(Duration::from_secs(2)).unwrap();
    // Two old flush intervals with no commands. A polling regression wakes
    // this observation early at 1,000 iterations; the fixed worker stays asleep.
    let _ = observed.recv_timeout(FLUSH_INTERVAL * 2);
    let idle_waits = waits.load(Ordering::SeqCst);
    let idle_zero_waits = zero_waits.load(Ordering::SeqCst);
    recorder.finish(Duration::from_secs(2)).unwrap();
    assert_eq!(
        idle_waits, 1,
        "idle receive attempts; zero-duration polls: {idle_zero_waits}"
    );
    assert_eq!(idle_zero_waits, 0);
}

struct LifecycleSink {
    bytes: Arc<Mutex<Vec<u8>>>,
    syncs: Arc<AtomicUsize>,
    flushed: SyncSender<()>,
    sleeping: SyncSender<()>,
}

impl Write for LifecycleSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = self.flushed.try_send(());
        Ok(())
    }
}

impl TranscriptSink for LifecycleSink {
    fn before_receive(&mut self, timeout: Option<Duration>) {
        if timeout.is_none() {
            let _ = self.sleeping.try_send(());
        }
    }

    fn sync_data(&mut self) -> std::io::Result<()> {
        self.syncs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct LifecycleFixture {
    recorder: TranscriptRecorder,
    bytes: Arc<Mutex<Vec<u8>>>,
    syncs: Arc<AtomicUsize>,
    flushed: Receiver<()>,
    sleeping: Receiver<()>,
}

impl LifecycleFixture {
    fn new(max_file_bytes: usize) -> Self {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let syncs = Arc::new(AtomicUsize::new(0));
        let (flushed_tx, flushed) = mpsc::sync_channel(8);
        let (sleeping_tx, sleeping) = mpsc::sync_channel(8);
        let sink = LifecycleSink {
            bytes: Arc::clone(&bytes),
            syncs: Arc::clone(&syncs),
            flushed: flushed_tx,
            sleeping: sleeping_tx,
        };
        let recorder = TranscriptRecorder::with_factory(
            PathBuf::from("writer-flush-lifecycle-fixture.jsonl"),
            metadata_entry(None, None, "synthetic", "default"),
            RecorderLimits {
                max_file_bytes,
                ..RecorderLimits::default()
            },
            Box::new(move || Ok(Box::new(sink))),
            None,
        )
        .unwrap();
        // Metadata is already flushed; the worker is entering its first
        // untimed receive. No filesystem or provider is involved.
        flushed.recv_timeout(Duration::from_secs(2)).unwrap();
        sleeping.recv_timeout(Duration::from_secs(2)).unwrap();
        Self {
            recorder,
            bytes,
            syncs,
            flushed,
            sleeping,
        }
    }

    fn await_periodic_flush(&self) {
        self.flushed.recv_timeout(Duration::from_secs(2)).unwrap();
        self.sleeping.recv_timeout(Duration::from_secs(2)).unwrap();
    }
}

#[test]
fn periodic_flush_returns_to_sleep_and_later_commands_wake_the_writer() {
    let mut fixture = LifecycleFixture::new(DEFAULT_MAX_FILE_BYTES);
    fixture.recorder.record_output("first batch").unwrap();
    fixture.await_periodic_flush();
    // An explicit flush is not required to trigger the periodic flush.
    // Afterward, neither repeated flushes nor timer wakeups should occur.
    assert!(matches!(
        fixture.flushed.recv_timeout(FLUSH_INTERVAL * 2),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(matches!(
        fixture.sleeping.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    let flush = fixture.recorder.flush().unwrap();
    let flushed = flush
        .reply
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    fixture.await_periodic_flush(); // This flush was explicitly requested.
    let bytes = fixture.bytes.lock().unwrap().clone();
    assert_eq!(flushed, bytes.len() as u64);
    assert!(String::from_utf8(bytes).unwrap().contains("first batch"));

    fixture
        .recorder
        .record_output("second batch after idle")
        .unwrap();
    fixture.await_periodic_flush();
    fixture.recorder.finish(Duration::from_secs(2)).unwrap();
    assert_eq!(fixture.syncs.load(Ordering::SeqCst), 1);
    let bytes = fixture.bytes.lock().unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert_eq!(text.matches("first batch").count(), 1);
    assert_eq!(text.matches("second batch after idle").count(), 1);
}

fn check_ongoing_traffic(capped: bool) {
    let limit = if capped { 1024 } else { DEFAULT_MAX_FILE_BYTES };
    let mut fixture = LifecycleFixture::new(limit);
    fixture
        .recorder
        .record_output("accepted before cap")
        .unwrap();
    if capped {
        fixture.recorder.record_output("x".repeat(2048)).unwrap();
    }
    fixture.recorder.record_output("ongoing traffic").unwrap();

    // Keep offering commands across the batch deadline. Any cap notice is
    // intentionally not polled yet, so the producer keeps enqueueing.
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut flushed_during_traffic = false;
    while Instant::now() < deadline {
        if fixture.flushed.try_recv().is_ok() {
            flushed_during_traffic = true;
            break;
        }
        fixture.recorder.record_output("ongoing traffic").unwrap();
        std::thread::sleep(Duration::from_millis(1));
    }
    // Completion cleans up even when an assertion would reject a regression.
    fixture.recorder.finish(Duration::from_secs(2)).unwrap();
    assert!(
        flushed_during_traffic,
        "ongoing traffic starved the periodic flush (capped={capped})"
    );
    assert_eq!(fixture.syncs.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.recorder.queued_bytes_for_diagnostics(), 0);
    let notices = fixture.recorder.poll_notices();
    if capped {
        assert!(matches!(
            notices.as_slice(),
            [TranscriptNotice::CapReached { .. }]
        ));
    } else {
        assert!(notices.is_empty());
    }
    let bytes = fixture.bytes.lock().unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("accepted before cap"));
    assert_eq!(text.contains("ongoing traffic"), !capped);
    assert!(bytes.len() <= limit);
}

#[test]
fn cap_rejected_traffic_cannot_starve_a_pending_batch_flush() {
    check_ongoing_traffic(true);
}

#[test]
fn accepted_traffic_cannot_postpone_the_first_entry_batch_deadline() {
    check_ongoing_traffic(false);
}
