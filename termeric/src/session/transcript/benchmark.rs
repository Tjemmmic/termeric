//! The transcript enqueue benchmark: a recorder whose sink is much slower
//! than its producer.

use super::*;

pub(crate) struct TranscriptEnqueueBenchmark {
    pub samples_ms: Vec<f64>,
    pub dropped_entries: Vec<f64>,
}

struct DelayedDiscardSink;

impl Write for DelayedDiscardSink {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(2));
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::thread::sleep(Duration::from_millis(2));
        Ok(())
    }
}

impl TranscriptSink for DelayedDiscardSink {}

/// Measure the terminal-facing enqueue path while the injected persistence
/// backend is much slower than the producer. The queue must shed load rather
/// than transferring the sink delay to input or PTY processing.
pub(crate) fn benchmark_slow_transcript_enqueue() -> std::io::Result<TranscriptEnqueueBenchmark> {
    const SAMPLES: usize = 5;
    const ENTRIES: usize = 1_000;
    let payload = "x".repeat(4_096);
    let mut samples_ms = Vec::with_capacity(SAMPLES);
    let mut dropped_entries = Vec::with_capacity(SAMPLES);

    for sample in 0..SAMPLES {
        let mut recorder = TranscriptRecorder::with_factory(
            PathBuf::from(format!("benchmark-{sample}.jsonl")),
            metadata_entry(None, None, "benchmark", "default"),
            RecorderLimits {
                max_queue_bytes: 64 * 1024,
                queue_entries: 16,
                ..RecorderLimits::default()
            },
            Box::new(|| Ok(Box::new(DelayedDiscardSink))),
            None,
        )?;
        let started = Instant::now();
        let mut dropped = 0usize;
        for entry in 0..ENTRIES {
            let result = if entry % 10 == 0 {
                recorder.record_input("synthetic input")?
            } else {
                recorder.record_output(&payload)?
            };
            if matches!(
                result,
                TranscriptWrite::Backpressure | TranscriptWrite::DroppedAtBackpressure
            ) {
                dropped += 1;
            }
        }
        samples_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
        dropped_entries.push(dropped as f64);
        assert!(
            dropped > 0,
            "slow persistence fixture must apply backpressure"
        );
        recorder.finish(Duration::from_secs(2))?;
    }

    Ok(TranscriptEnqueueBenchmark {
        samples_ms,
        dropped_entries,
    })
}
