use super::*;
use std::sync::Mutex;

struct BlockedSyncSink {
    entered: mpsc::Sender<()>,
    release: Receiver<()>,
    block_on_drop: bool,
}

impl Write for BlockedSyncSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TranscriptSink for BlockedSyncSink {
    fn sync_data(&mut self) -> std::io::Result<()> {
        if !self.block_on_drop {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
        }
        Ok(())
    }
}

impl Drop for BlockedSyncSink {
    fn drop(&mut self) {
        if self.block_on_drop {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
        }
    }
}

fn check_blocked_shutdown(block_on_drop: bool) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut recorder = TranscriptRecorder::with_factory(
        PathBuf::from("blocked-sync-fixture.jsonl"),
        metadata_entry(None, None, "synthetic", "default"),
        RecorderLimits::default(),
        Box::new(move || {
            Ok(Box::new(BlockedSyncSink {
                entered: entered_tx,
                release: release_rx,
                block_on_drop,
            }))
        }),
        None,
    )
    .unwrap();
    let first = recorder.finish(Duration::from_millis(5));
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let retry = recorder.finish(Duration::from_millis(5));
    // Release and join before assertions so the red test cannot leak a worker.
    release_tx.send(()).unwrap();
    recorder.worker.take().unwrap().join().unwrap();
    assert_eq!(first.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(retry.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    recorder.finish(Duration::from_secs(1)).unwrap();
    recorder.finish(Duration::ZERO).unwrap();
}

#[test]
fn a_shutdown_retry_cannot_succeed_while_durable_sync_is_still_blocked() {
    check_blocked_shutdown(false);
}

#[test]
fn shutdown_timeout_includes_sink_destruction_without_an_unbounded_join() {
    check_blocked_shutdown(true);
}

struct RecordingSink {
    bytes: Arc<Mutex<Vec<u8>>>,
    syncs: Arc<AtomicUsize>,
    fail_sync: bool,
}

impl Write for RecordingSink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TranscriptSink for RecordingSink {
    fn sync_data(&mut self) -> std::io::Result<()> {
        self.syncs.fetch_add(1, Ordering::SeqCst);
        if self.fail_sync {
            Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "synthetic sync failure",
            ))
        } else {
            Ok(())
        }
    }
}

#[test]
fn full_queue_drains_once_and_final_text_loss_survives_timeout_retries() {
    for unfinished_input in [false, true] {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let syncs = Arc::new(AtomicUsize::new(0));
        let sink = RecordingSink {
            bytes: Arc::clone(&bytes),
            syncs: Arc::clone(&syncs),
            fail_sync: false,
        };
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("full-shutdown-queue-fixture.jsonl"),
            metadata_entry(None, None, "synthetic", "default"),
            RecorderLimits {
                queue_entries: 1,
                ..RecorderLimits::default()
            },
            Box::new(move || {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(Box::new(sink))
            }),
            None,
        )
        .unwrap();
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(
            recorder.record_output("accepted output").unwrap(),
            TranscriptWrite::Written
        );
        if unfinished_input {
            assert_eq!(
                recorder.record_input(b"\xe2").unwrap(),
                TranscriptWrite::Written
            );
        }
        let first = recorder.finish(Duration::ZERO);
        let retry = recorder.finish(Duration::ZERO);
        let refused = recorder.record_output("refused after shutdown");
        release_tx.send(()).unwrap();
        recorder.worker.take().unwrap().join().unwrap();
        assert_eq!(first.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(retry.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(refused.unwrap_err().kind(), std::io::ErrorKind::BrokenPipe);
        for _ in 0..2 {
            let result = recorder.finish(Duration::ZERO);
            if unfinished_input {
                assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
            } else {
                result.unwrap();
            }
        }
        assert_eq!(syncs.load(Ordering::SeqCst), 1);
        assert_eq!(recorder.queued_bytes_for_diagnostics(), 0);
        let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
        assert_eq!(text.matches("accepted output").count(), 1);
        assert!(!text.contains("refused after shutdown"));
    }
}

#[test]
fn shutdown_failures_remain_errors_after_notices_are_consumed() {
    for startup_failure in [false, true] {
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from("failed-shutdown-fixture.jsonl"),
            metadata_entry(None, None, "synthetic", "default"),
            RecorderLimits::default(),
            Box::new(move || {
                if startup_failure {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "synthetic open failure",
                    ))
                } else {
                    Ok(Box::new(RecordingSink {
                        bytes: Arc::new(Mutex::new(Vec::new())),
                        syncs: Arc::new(AtomicUsize::new(0)),
                        fail_sync: true,
                    }))
                }
            }),
            None,
        )
        .unwrap();
        if startup_failure {
            recorder.worker.take().unwrap().join().unwrap();
        }
        recorder.poll_notices();
        let first = recorder.finish(Duration::from_secs(2)).unwrap_err();
        recorder.poll_notices();
        let retry = recorder.finish(Duration::ZERO).unwrap_err();
        assert_eq!(
            first.kind(),
            if startup_failure {
                std::io::ErrorKind::PermissionDenied
            } else {
                std::io::ErrorKind::StorageFull
            }
        );
        assert_eq!(retry.kind(), first.kind());
        assert_eq!(retry.to_string(), first.to_string());
    }
}

#[test]
fn a_panicked_writer_never_becomes_successful_on_retry() {
    let mut recorder = TranscriptRecorder::with_factory(
        PathBuf::from("panicked-shutdown-fixture.jsonl"),
        metadata_entry(None, None, "synthetic", "default"),
        RecorderLimits::default(),
        Box::new(|| panic!("synthetic writer panic")),
        None,
    )
    .unwrap();
    assert!(recorder.worker.take().unwrap().join().is_err());
    for _ in 0..2 {
        assert_eq!(
            recorder.finish(Duration::ZERO).unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    }
}
