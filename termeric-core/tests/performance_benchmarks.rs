//! Release-only benchmark workloads consumed by scripts/run_benchmarks.py.

use serde::Serialize;
use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use termeric_core::graphics::ImageStorage;
use termeric_core::pty::{PtyEvent, spawn_reader_thread};
use termeric_core::{ImageId, Pty, StoredImage, Terminal};

const SAMPLE_COUNT: usize = 5;

struct CountingAllocator;

static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: This forwards the allocator contract and returns System's
        // result unchanged; accounting occurs only after successful allocation.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: This forwards the allocator contract and returns System's
        // result unchanged; accounting occurs only after successful allocation.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` and `layout` are the pair supplied by the caller
        // under GlobalAlloc's contract and are forwarded unchanged.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: This forwards the allocator contract and returns System's
        // result unchanged; requested replacement bytes are counted only when
        // the reallocation succeeds.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Debug)]
struct AllocationSnapshot {
    bytes: u64,
    count: u64,
}

fn allocation_snapshot() -> AllocationSnapshot {
    AllocationSnapshot {
        bytes: ALLOCATED_BYTES.load(Ordering::Relaxed),
        count: ALLOCATION_COUNT.load(Ordering::Relaxed),
    }
}

fn allocation_delta(before: AllocationSnapshot) -> AllocationSnapshot {
    let after = allocation_snapshot();
    AllocationSnapshot {
        bytes: after.bytes.saturating_sub(before.bytes),
        count: after.count.saturating_sub(before.count),
    }
}

#[derive(Serialize)]
struct Metric<'a> {
    name: &'a str,
    unit: &'a str,
    samples: Vec<f64>,
    workload: &'a str,
}

fn emit(name: &'static str, unit: &'static str, samples: Vec<f64>, workload: &'static str) {
    println!(
        "TERMERIC_BENCH {}",
        serde_json::to_string(&Metric {
            name,
            unit,
            samples,
            workload,
        })
        .unwrap()
    );
}

fn elapsed_ms(operation: impl FnOnce()) -> f64 {
    let started = Instant::now();
    operation();
    started.elapsed().as_secs_f64() * 1_000.0
}

fn output_fixture(lines: usize) -> Vec<u8> {
    let mut output = String::with_capacity(lines * 52);
    for line in 0..lines {
        writeln!(
            output,
            "\x1b[3{}m{:06} output αβ界 https://example.com/{}\x1b[0m\r",
            line % 8,
            line,
            line
        )
        .unwrap();
    }
    output.into_bytes()
}

fn current_rss_kib() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

fn representation_bytes_per_line(input: &[u8], lines: usize) -> Vec<f64> {
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let mut terminal = Terminal::new(120, 40);
        terminal.set_scrollback_limits(lines, 256 * 1024 * 1024);
        terminal.process(input);
        let usage = terminal.memory_usage();
        samples.push(usage.scrollback_bytes as f64 / usage.scrollback_lines.max(1) as f64);
    }
    samples
}

#[cfg(debug_assertions)]
fn require_release_profile() {
    panic!("performance suite requires --release");
}

#[cfg(not(debug_assertions))]
fn require_release_profile() {}

#[test]
fn counting_allocator_observes_requested_bytes_and_calls() {
    let before = allocation_snapshot();
    let allocation = std::hint::black_box(vec![0_u8; 4_096]);
    let delta = allocation_delta(before);
    assert!(delta.bytes >= allocation.len() as u64);
    assert!(delta.count >= 1);
}

#[test]
#[ignore = "run with scripts/run_benchmarks.py; requires a release build"]
fn release_benchmark_suite() {
    require_release_profile();

    let plain = vec![b'x'; 4 * 1024 * 1024];
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let mut terminal = Terminal::new(120, 40);
        samples.push(elapsed_ms(|| terminal.process(&plain)));
        assert!(terminal.grid().cursor().row < terminal.rows());
    }
    emit(
        "core.parser.plain_4mib",
        "ms",
        samples,
        "4 MiB printable ASCII",
    );

    let escape_heavy =
        b"\x1b[38;2;12;34;56mcolor\x1b[0m\r\n\x1b]8;;https://example.com\x07link\x1b]8;;\x07\r\n"
            .repeat(32_768);
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let mut terminal = Terminal::new(120, 40);
        samples.push(elapsed_ms(|| terminal.process(&escape_heavy)));
        assert!(terminal.hyperlink_registry().len() <= 1);
    }
    emit(
        "core.parser.escape_heavy",
        "ms",
        samples,
        "2.4 MiB truecolor, OSC 8, CRLF",
    );

    let ten_thousand = output_fixture(10_000);
    let mut ten_k_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut ten_k_rss = Vec::with_capacity(SAMPLE_COUNT);
    let mut ten_k_estimated = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let before = current_rss_kib().unwrap_or(0.0);
        let mut terminal = Terminal::new(120, 40);
        terminal.grid_mut().set_scrollback_limit(10_000);
        ten_k_times.push(elapsed_ms(|| terminal.process(&ten_thousand)));
        assert!(terminal.grid().scrollback_len() <= 10_000);
        ten_k_estimated.push(terminal.memory_usage().scrollback_bytes as f64);
        ten_k_rss.push((current_rss_kib().unwrap_or(before) - before).max(0.0));
        drop(terminal);
    }
    emit(
        "core.scrollback.10k_time",
        "ms",
        ten_k_times,
        "10,000 styled 120-column lines",
    );
    emit(
        "memory.scrollback.10k_rss_delta",
        "KiB",
        ten_k_rss,
        "process RSS delta while 10,000-line terminal is live",
    );
    emit(
        "memory.scrollback.10k_estimated_bytes",
        "bytes",
        ten_k_estimated,
        "compact row-owned bytes for 10,000 styled lines",
    );

    let hundred_thousand = output_fixture(100_000);
    let mut hundred_k_times = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_rss = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_estimated = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_graphemes = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_styles = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_hyperlinks = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_allocated_bytes = Vec::with_capacity(SAMPLE_COUNT);
    let mut hundred_k_allocation_count = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let before = current_rss_kib().unwrap_or(0.0);
        let allocation_before = allocation_snapshot();
        let mut terminal = Terminal::new(120, 40);
        terminal.set_scrollback_limits(100_000, 128 * 1024 * 1024);
        hundred_k_times.push(elapsed_ms(|| terminal.process(&hundred_thousand)));
        assert!(terminal.grid().scrollback_len() <= 100_000);
        assert!(terminal.grid().scrollback_len() >= 99_000);
        assert!(terminal.rows_text(0, 39).contains("99999"));
        let usage = terminal.memory_usage();
        hundred_k_estimated.push(usage.scrollback_bytes as f64);
        hundred_k_graphemes.push(usage.grapheme_bytes as f64);
        hundred_k_styles.push(usage.style_bytes as f64);
        hundred_k_hyperlinks.push(usage.hyperlink_bytes as f64);
        hundred_k_rss.push((current_rss_kib().unwrap_or(before) - before).max(0.0));
        let allocation = allocation_delta(allocation_before);
        hundred_k_allocated_bytes.push(allocation.bytes as f64);
        hundred_k_allocation_count.push(allocation.count as f64);
        drop(terminal);
    }
    emit(
        "core.scrollback.100k_time",
        "ms",
        hundred_k_times,
        "100,000 styled 120-column lines",
    );
    emit(
        "memory.scrollback.100k_rss_delta",
        "KiB",
        hundred_k_rss,
        "process RSS delta while 100,000-line terminal is live",
    );
    emit(
        "memory.scrollback.100k_estimated_bytes",
        "bytes",
        hundred_k_estimated,
        "compact row-owned bytes for 100,000 styled lines",
    );
    emit(
        "memory.scrollback.100k_grapheme_bytes",
        "bytes",
        hundred_k_graphemes,
        "heap bytes owned by multi-codepoint graphemes",
    );
    emit(
        "memory.scrollback.100k_style_bytes",
        "bytes",
        hundred_k_styles,
        "bytes owned by scrollback style runs",
    );
    emit(
        "memory.scrollback.100k_hyperlink_bytes",
        "bytes",
        hundred_k_hyperlinks,
        "hyperlink references and deduplicated registry bytes",
    );
    emit(
        "memory.scrollback.100k_allocated_bytes",
        "bytes",
        hundred_k_allocated_bytes,
        "allocator bytes requested while constructing and processing 100,000 styled lines",
    );
    emit(
        "memory.scrollback.100k_allocation_count",
        "allocations",
        hundred_k_allocation_count,
        "successful allocator calls while constructing and processing 100,000 styled lines",
    );

    let representation_lines = 1_000;
    let plain_rows = "plain terminal output\r\n".repeat(representation_lines);
    emit(
        "memory.representation.plain_bytes_per_line",
        "bytes/line",
        representation_bytes_per_line(plain_rows.as_bytes(), representation_lines),
        "1,000 plain rows",
    );
    let styled_rows = output_fixture(representation_lines);
    emit(
        "memory.representation.styled_bytes_per_line",
        "bytes/line",
        representation_bytes_per_line(&styled_rows, representation_lines),
        "1,000 ANSI styled rows",
    );
    let unicode_rows = "e\u{301} αβ界 👩\u{200d}💻\r\n".repeat(representation_lines);
    emit(
        "memory.representation.unicode_bytes_per_line",
        "bytes/line",
        representation_bytes_per_line(unicode_rows.as_bytes(), representation_lines),
        "1,000 combining, wide, and ZWJ rows",
    );
    let hyperlink_rows =
        "\x1b]8;;https://example.com/shared\x07linked\x1b]8;;\x07\r\n".repeat(representation_lines);
    emit(
        "memory.representation.hyperlink_bytes_per_line",
        "bytes/line",
        representation_bytes_per_line(hyperlink_rows.as_bytes(), representation_lines),
        "1,000 rows sharing one OSC 8 hyperlink",
    );
    let sparse_rows = "x\r\n".repeat(representation_lines);
    emit(
        "memory.representation.sparse_bytes_per_line",
        "bytes/line",
        representation_bytes_per_line(sparse_rows.as_bytes(), representation_lines),
        "1,000 one-cell rows at 120 columns",
    );

    let reflow_seed = output_fixture(10_000);
    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut reflow_allocated_bytes = Vec::with_capacity(SAMPLE_COUNT);
    let mut reflow_allocation_count = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let mut terminal = Terminal::new(120, 40);
        terminal.grid_mut().set_scrollback_limit(20_000);
        terminal.process(&reflow_seed);
        let allocation_before = allocation_snapshot();
        samples.push(elapsed_ms(|| {
            for iteration in 0..50 {
                terminal.resize(80 + iteration % 81, 30 + iteration % 21);
            }
        }));
        let allocation = allocation_delta(allocation_before);
        reflow_allocated_bytes.push(allocation.bytes as f64);
        reflow_allocation_count.push(allocation.count as f64);
        assert!(terminal.grid().cursor().row < terminal.rows());
    }
    emit(
        "core.reflow.10k_50_resizes",
        "ms",
        samples,
        "10,000-line history and 50 alternating resizes",
    );
    emit(
        "memory.reflow.10k_50_resizes_allocated_bytes",
        "bytes",
        reflow_allocated_bytes,
        "allocator bytes requested by 50 resizes of a 10,000-line history",
    );
    emit(
        "memory.reflow.10k_50_resizes_allocation_count",
        "allocations",
        reflow_allocation_count,
        "successful allocator calls during 50 resizes of a 10,000-line history",
    );

    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let mut storage = ImageStorage::new(256, 64 * 1024 * 1024);
        samples.push(elapsed_ms(|| {
            for image in 0..64 {
                storage
                    .store(StoredImage {
                        id: ImageId(image),
                        width: 256,
                        height: 256,
                        data: vec![image as u8; 256 * 256 * 4].into(),
                        revision: image as u64,
                    })
                    .unwrap();
            }
        }));
        assert_eq!(storage.len(), 64);
        assert_eq!(storage.memory_usage(), 16 * 1024 * 1024);
    }
    emit(
        "core.images.store_64_rgba",
        "ms",
        samples,
        "store 64 raw 256x256 RGBA images under the 64 MiB cap",
    );

    if std::path::Path::new("/bin/cat").exists() {
        let mut samples = Vec::with_capacity(SAMPLE_COUNT);
        for index in 0..SAMPLE_COUNT {
            let pty = Pty::new_with_command(80, 24, Some("/bin/cat"), &[]).unwrap();
            let (mut handle, reader) = pty.take_reader();
            let receiver = spawn_reader_thread(reader);
            let payload = format!("termeric-latency-{index}\n");
            let started = Instant::now();
            handle.write_all(payload.as_bytes()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut observed = Vec::new();
            while Instant::now() < deadline
                && !String::from_utf8_lossy(&observed).contains(payload.trim())
            {
                match receiver.recv_timeout(Duration::from_millis(20)) {
                    Ok(PtyEvent::Output(bytes)) => observed.extend(bytes),
                    Ok(PtyEvent::Closed | PtyEvent::Error(_))
                    | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
            assert!(String::from_utf8_lossy(&observed).contains(payload.trim()));
            samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        }
        emit(
            "latency.pty_cat_roundtrip",
            "ms",
            samples,
            "write through a real PTY and observe /bin/cat output",
        );
    }
}
