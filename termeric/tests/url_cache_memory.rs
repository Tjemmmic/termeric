//! Isolated allocation probe for the production URL cache's hot scan path.

#[allow(dead_code)]
#[path = "../src/terminal/url.rs"]
mod url;

#[path = "../src/terminal/row_text.rs"]
mod row_text;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

struct CountingAllocator;

thread_local! {
    static COUNTS: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

fn allocated(bytes: usize) {
    let _ = COUNTS.try_with(|counts| {
        if let Some((calls, total)) = counts.get() {
            counts.set(Some((calls + 1, total + bytes)));
        }
    });
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the caller's allocation contract unchanged.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the caller's allocation contract unchanged.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            allocated(layout.size());
        }
        pointer
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: Forward the original pointer, layout, and requested size.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        if !replacement.is_null() {
            allocated(size);
        }
        replacement
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: Forward the caller's original allocation pair unchanged.
        unsafe { System.dealloc(pointer, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn measure(f: impl FnOnce()) -> (usize, usize) {
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            COUNTS.set(None);
        }
    }
    COUNTS.set(Some((0, 0)));
    let _stop = Stop;
    f();
    COUNTS.get().unwrap()
}

#[test]
fn mixed_case_scheme_scanning_does_not_allocate_a_normalized_line() {
    let detector = url::UrlDetector::new();
    let text = format!("{} HtTpS://example.test/Case/%2f", "hF界mSg".repeat(10_000));
    let (calls, bytes) = measure(|| {
        let urls = detector.detect_in_line(black_box(&text), 0);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url, "HtTpS://example.test/Case/%2f");
    });
    assert!(calls > 0, "the probe must observe the result allocations");
    assert!(
        bytes < 1024,
        "scanning scratch must not scale with line size: {bytes}"
    );
}

#[test]
fn hot_wrapped_ascii_url_mapping_has_a_small_allocation_budget() {
    let mut terminal = termeric_core::Terminal::new(100, 30);
    let target = format!(
        "https://e.test/{}",
        "x".repeat(3_000 - "https://e.test/".len())
    );
    terminal.process(target.as_bytes());
    let detector = url::UrlDetector::new();
    let mut cache = url::UrlCache::new(0);
    cache.update_grid(terminal.grid(), &detector);
    assert_eq!(cache.url_at(29, 99).unwrap().url, target);
    let before = cache.metrics();
    let (calls, bytes) = measure(|| {
        for _ in 0..10 {
            cache.update_grid(black_box(terminal.grid()), black_box(&detector));
            black_box(&cache);
        }
    });
    eprintln!(
        "100x30 wrapped ASCII, 10 hot scans: {calls} allocations, {bytes} cumulative requested bytes"
    );
    assert_eq!(cache.metrics(), (before.0, before.1 + 10));
    assert!(
        calls > 0,
        "the probe must count actual scan scratch storage"
    );
    assert!(
        bytes <= 10 * 32 * 1024,
        "hot ASCII mapping should not allocate a cell-sized span table: {bytes}"
    );
}
