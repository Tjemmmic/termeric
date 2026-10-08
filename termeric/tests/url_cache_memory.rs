//! Isolated allocation probe for the production URL cache's hot scan path.

#[allow(dead_code)]
#[path = "../src/terminal/url.rs"]
mod url;

#[path = "../src/terminal/row_text.rs"]
mod row_text;

mod common;

use common::allocations::measure;
use std::hint::black_box;

#[global_allocator]
static ALLOCATOR: common::allocations::CountingAllocator = common::allocations::CountingAllocator;

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
