//! Synthetic observer-only release workloads; never launch a provider or PTY.

use super::{DiffDetector, DiffLine, MAX_HISTORY_BYTES, MAX_HISTORY_COUNT};
use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Instant;

pub(crate) struct DiffBenchmark {
    pub plain_ms: Vec<f64>,
    pub bulk_ms: Vec<f64>,
    pub fragmented_ms: Vec<f64>,
    pub retained_bytes: Vec<f64>,
}

struct Fixture {
    wire: Vec<u8>,
    raw_bytes: usize,
    last_raw: String,
    large_line: String,
}

impl Fixture {
    fn new() -> Self {
        let mut wire = String::new();
        let mut raw_bytes = 0;
        let mut last_raw = String::new();
        let large_line = "界".repeat(340);
        for patch in 0..80 {
            let (lines, content) = if patch < 64 {
                (1, "small 界")
            } else {
                (200, large_line.as_str())
            };
            let mut raw =
                format!("--- a/bench-{patch}\n+++ b/bench-{patch}\n@@ -0,0 +1,{lines} @@\n");
            for _ in 0..lines {
                writeln!(raw, "+{content}").unwrap();
            }
            raw_bytes += raw.len();
            // Every header/data line is decorated and uses CRLF. Fixed chunks
            // cut through UTF-8, escape sequences, headers and data alike.
            for line in raw.lines() {
                write!(wire, "\x1b[32m{line}\x1b[0m\r\n").unwrap();
            }
            wire.push_str("\x1b]0;synthetic diff benchmark\x1b\\shell prompt\r\n");
            last_raw = raw;
        }
        Self {
            wire: wire.into_bytes(),
            raw_bytes,
            last_raw,
            large_line,
        }
    }

    fn measure(&self, chunk_size: usize) -> (f64, usize) {
        let mut detector = DiffDetector::new();
        let mut count = 0;
        let mut id_sum = 0;
        let mut raw_bytes = 0;
        // No formatting, diagnostics or raw-patch cloning in the timed path.
        // Constant-cost completion counters keep finalization observable even
        // when the patch has already been evicted by the end of the stream.
        let mut completed = |diff: &super::DetectedDiff| {
            count += 1;
            id_sum += diff.id;
            raw_bytes += diff.raw.len();
        };
        let started = Instant::now();
        for chunk in self.wire.chunks(chunk_size) {
            detector.feed_bytes(black_box(chunk), &mut completed);
        }
        detector.finish_stream(&mut completed);
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;

        assert_eq!((count, id_sum, raw_bytes), (80, 3240, self.raw_bytes));
        let (retained, bytes) = detector.history_diagnostics();
        assert!(retained > 0 && retained < MAX_HISTORY_COUNT);
        assert!((3 * 1024 * 1024..=MAX_HISTORY_BYTES).contains(&bytes));
        assert!(detector.detected_by_id(1).is_none());
        let last = detector.latest_unreviewed().unwrap();
        assert_eq!(last.id, 80);
        assert_eq!(last.filename, "bench-79");
        assert_eq!(last.raw, self.last_raw);
        assert_eq!(last.hunks.len(), 1);
        assert_eq!(last.hunks[0].lines.len(), 200);
        assert!(
            last.hunks[0]
                .lines
                .iter()
                .all(|line| matches!(line, DiffLine::Added(text) if text == &self.large_line))
        );
        let notices: Vec<_> = detector.take_notices().collect();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].contains("evicted"));
        black_box(&detector);
        (elapsed, bytes)
    }
}

fn measure_plain(wire: &[u8]) -> f64 {
    let mut detector = DiffDetector::new();
    let initial_history = detector.history_diagnostics();
    let mut completions = 0;
    let started = Instant::now();
    for chunk in wire.chunks(4096) {
        detector.feed_bytes(black_box(chunk), |_| completions += 1);
    }
    detector.finish_stream(|_| completions += 1);
    let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(completions, 0);
    assert_eq!(detector.history_diagnostics(), initial_history);
    assert_eq!(detector.take_notices().count(), 0);
    black_box(&detector);
    elapsed
}

pub(crate) fn benchmark_streaming_diff() -> DiffBenchmark {
    let fixture = Fixture::new();
    let mut line = vec![b'x'; 127];
    line.push(b'\n');
    let plain = line.repeat(32_768);
    assert_eq!(plain.len(), 4 * 1024 * 1024);
    let mut result = DiffBenchmark {
        plain_ms: Vec::with_capacity(5),
        bulk_ms: Vec::with_capacity(5),
        fragmented_ms: Vec::with_capacity(5),
        retained_bytes: Vec::with_capacity(5),
    };
    // One unrecorded warmup, then five independent detectors per workload.
    // Alternate the paired read sizes to avoid always favoring the same order.
    for sample in 0..6 {
        let plain_ms = measure_plain(&plain);
        let (bulk, fragmented) = if sample % 2 == 0 {
            (fixture.measure(4096), fixture.measure(257))
        } else {
            let fragmented = fixture.measure(257);
            (fixture.measure(4096), fragmented)
        };
        assert_eq!(bulk.1, fragmented.1);
        if sample != 0 {
            result.plain_ms.push(plain_ms);
            result.bulk_ms.push(bulk.0);
            result.fragmented_ms.push(fragmented.0);
            result.retained_bytes.push(bulk.1 as f64);
        }
    }
    result
}

#[test]
fn release_diff_fixture_preserves_completed_content_and_evicts_under_fragmentation() {
    let fixture = Fixture::new();
    let bulk = fixture.measure(4096);
    let fragmented = fixture.measure(257);
    assert_eq!(bulk.1, fragmented.1);
    measure_plain(b"ordinary output\r\nnot a patch\n");
}
