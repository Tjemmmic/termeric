//! Allocation and exact-geometry regression for the production query viewport.
//! This integration binary's allocator is isolated from the application.

#[path = "../src/ui/text_input_viewport.rs"]
mod text_input_viewport;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use text_input_viewport::TextInputViewport;
use unicode_segmentation::UnicodeSegmentation;

struct CountingAllocator;

thread_local! {
    // Only this test thread's measured region is counted, not harness activity.
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
        // SAFETY: Forward the caller's pointer, layout, and replacement size.
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

// Frozen pre-optimization implementation. Keep independent of the new traversal.
fn reference(text: &str, cursor: usize, columns: usize) -> TextInputViewport {
    let graphemes: Vec<_> = text.grapheme_indices(true).collect();
    let caret = graphemes.partition_point(|(offset, grapheme)| offset + grapheme.len() <= cursor);
    let mut start = caret;
    let mut cursor_columns = 0;
    while start > 0 {
        let width = termeric_core::unicode::grapheme_width(graphemes[start - 1].1);
        if cursor_columns + width > columns - 1 {
            break;
        }
        cursor_columns += width;
        start -= 1;
    }
    let start_byte = graphemes
        .get(start)
        .map_or(text.len(), |(offset, _)| *offset);
    let mut end_byte = start_byte;
    let mut used = 0;
    for (offset, grapheme) in &graphemes[start..] {
        let width = termeric_core::unicode::grapheme_width(grapheme);
        if used + width > columns {
            break;
        }
        used += width;
        end_byte = offset + grapheme.len();
    }
    TextInputViewport {
        text_range: start_byte..end_byte,
        text_x: 120.0,
        text_y: 55.0,
        cursor_x: 120.0 + cursor_columns as f32 * 9.5,
        right: 120.0 + columns as f32 * 9.5,
        columns,
    }
}

fn current(text: &str, cursor: usize, columns: usize) -> TextInputViewport {
    TextInputViewport::calculate(text, cursor, 120.0, 55.0, 120.0 + columns as f32 * 9.5, 9.5)
        .unwrap()
}

#[test]
fn streaming_viewport_matches_indexed_reference_at_every_scalar_caret() {
    let fixtures = [
        String::new(),
        "plain text".into(),
        "x".repeat(4096),
        format!("a{}", "\u{301}".repeat(2000)),
        "a界e\u{301}👩\u{200d}💻🇺🇸z".repeat(16),
        "\u{301}\u{200d}\u{fe0f}a\r\nb".into(),
        "🇦🇧🇨🇩🇪🇫🇬".repeat(8),
        "क्\u{200d}षक्ष각\u{600}a".repeat(8),
        "👩🏽\u{200d}💻1\u{fe0f}\u{20e3}\u{200b}".repeat(8),
    ];
    for text in &fixtures {
        for cursor in text
            .char_indices()
            .map(|(byte, _)| byte)
            .chain([text.len()])
        {
            for columns in [1, 2, 3, 5, 17, 80, 257, 5000] {
                let expected = reference(text, cursor, columns);
                let actual = current(text, cursor, columns);
                assert_eq!(actual, expected, "cursor={cursor}, columns={columns}");
                // The same output also gives identical pointer geometry.
                for x in [119.0, 125.0, actual.cursor_x, actual.right] {
                    assert_eq!(
                        actual.cursor_at(text, x, 9.5),
                        expected.cursor_at(text, x, 9.5)
                    );
                }
            }
        }
    }
}

#[test]
fn viewport_and_pointer_mapping_allocate_no_heap_storage() {
    for text in [
        "x".repeat(4096),
        "a界e\u{301}👩\u{200d}💻🇺🇸".repeat(128),
        format!("a{}", "\u{301}".repeat(2000)),
        "🇦🇧🇨🇩🇪🇫🇬".repeat(128),
    ] {
        let expected = reference(&text, text.len(), 80);
        let old = measure(|| {
            black_box(reference(black_box(&text), text.len(), 80));
        });
        let new = measure(|| {
            for _ in 0..100 {
                let view = current(black_box(&text), text.len(), 80);
                assert_eq!(view, expected);
                black_box(view.cursor_at(&text, 125.0, 9.5));
                black_box(view);
            }
        });
        assert!(
            old.0 > 0 && old.1 > 0,
            "reference must exercise the allocation probe"
        );
        assert_eq!(
            new,
            (0, 0),
            "100 viewport and pointer evaluations must allocate nothing"
        );
        println!(
            "{} query bytes: indexed reference {} allocations / {} cumulative requested bytes per call; streaming 0 / 0 across 100 calls",
            text.len(),
            old.0,
            old.1
        );
    }
}

#[test]
#[ignore = "opt-in paired viewport timing diagnostic; not a cross-machine budget"]
// Compile in debug builds but fail if explicitly invoked there. A const
// assertion would instead break the ordinary test build.
#[allow(clippy::assertions_on_constants)]
fn paired_query_viewport_cpu_benchmark() {
    use std::time::Instant;

    assert!(
        !cfg!(debug_assertions),
        "run this diagnostic with --release"
    );
    for (name, text) in [
        ("ascii", "x".repeat(4096)),
        ("mixed", "a界e\u{301}👩\u{200d}💻🇺🇸".repeat(128)),
        ("regional_indicators", "🇦🇧🇨🇩🇪🇫🇬".repeat(128)),
        ("combining_cluster", format!("a{}", "\u{301}".repeat(2000))),
        ("indic", "क्\u{200d}षक्ष".repeat(128)),
    ] {
        assert!(text.len() <= 4096);
        let middle = text.char_indices().nth(text.chars().count() / 2).unwrap().0;
        let cases: Vec<_> = [0, middle, text.len()]
            .into_iter()
            .flat_map(|cursor| [1, 80, 512].map(|columns| (cursor, columns)))
            .collect();
        for &(cursor, columns) in &cases {
            assert_eq!(
                current(&text, cursor, columns),
                reference(&text, cursor, columns)
            );
        }
        let run = |calculate: fn(&str, usize, usize) -> TextInputViewport| {
            let started = Instant::now();
            for _ in 0..50 {
                for &(cursor, columns) in &cases {
                    black_box(calculate(
                        black_box(&text),
                        black_box(cursor),
                        black_box(columns),
                    ));
                }
            }
            started.elapsed().as_secs_f64() * 1000.0
        };
        // Equal warmup, then alternate order to avoid always favoring one path.
        black_box(run(reference));
        black_box(run(current));
        let mut indexed_ms = Vec::new();
        let mut streaming_ms = Vec::new();
        for sample in 0..5 {
            if sample % 2 == 0 {
                indexed_ms.push(run(reference));
                streaming_ms.push(run(current));
            } else {
                streaming_ms.push(run(current));
                indexed_ms.push(run(reference));
            }
        }
        println!(
            "VIEWPORT_CPU {}",
            serde_json::json!({
                "fixture": name, "query_bytes": text.len(),
                "workload": "v1; 50 passes; carets=start,scalar-midpoint,end; columns=1,80,512",
                "indexed_ms": indexed_ms, "streaming_ms": streaming_ms,
            })
        );
    }
}
