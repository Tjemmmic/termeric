//! Allocation-traffic rail for row recycling, not a resident-memory benchmark.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use termeric_core::Terminal;
use termeric_core::grid::Grid;

thread_local! {
    static TRAFFIC: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn record(pointer: *mut u8, bytes: usize) {
    if !pointer.is_null() {
        let _ = TRAFFIC.try_with(|traffic| {
            if let Some((count, total)) = traffic.get() {
                traffic.set(Some((count + 1, total + bytes)));
            }
        });
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the unchanged caller contract to System.
        let pointer = unsafe { System.alloc(layout) };
        record(pointer, layout.size());
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: Forward the unchanged caller contract to System.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        record(pointer, layout.size());
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: Forward the original allocation pointer/layout pair.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: Forward the original allocation and requested size unchanged.
        let replacement = unsafe { System.realloc(pointer, layout, size) };
        record(replacement, size);
        replacement
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn nonarchiving_scrolls_reuse_rows_without_allocations() {
    let mut failures = Vec::new();
    for cols in [1, 80, 240] {
        for operation in ["reverse", "region_down", "region_up", "zero_history_up"] {
            let mut grid = Grid::new(cols, 30);
            grid.set_scrollback_limit(0);
            TRAFFIC.with(|traffic| traffic.set(Some((0, 0))));
            for _ in 0..1_000 {
                match operation {
                    "reverse" => grid.scroll_down(),
                    "region_down" => grid.scroll_region_down(2, 28),
                    "region_up" => {
                        grid.scroll_region_up(2, 28);
                    }
                    "zero_history_up" => {
                        grid.scroll_region_up(0, 28);
                    }
                    _ => unreachable!(),
                }
            }
            let measured = TRAFFIC.with(|traffic| traffic.replace(None).unwrap());
            eprintln!(
                "{operation}, {cols} columns, 1000 scrolls: {} allocations / {} requested bytes",
                measured.0, measured.1
            );
            if measured != (0, 0) {
                failures.push((operation, cols, measured));
            }
            assert_eq!(grid.scrollback_len(), 0);
        }
    }
    assert!(
        failures.is_empty(),
        "unexpected allocation traffic: {failures:?}"
    );
}

#[test]
fn colored_terminal_scrolls_preserve_allocation_free_row_reuse() {
    for operation in *b"STLM" {
        let mut terminal = Terminal::new(80, 30);
        // An alternate screen has no archival allocation. Exercise the actual
        // terminal-level BCE path, not just its underlying grid operations.
        terminal.process(b"\x1b[?1049h\x1b[44m");
        TRAFFIC.with(|traffic| traffic.set(Some((0, 0))));
        for _ in 0..1_000 {
            match operation {
                b'S' => terminal.scroll_up(1),
                b'T' => terminal.scroll_down(1),
                b'L' => terminal.insert_lines(1),
                b'M' => terminal.delete_lines(1),
                _ => unreachable!(),
            }
        }
        let measured = TRAFFIC.with(|traffic| traffic.replace(None).unwrap());
        assert_eq!(measured, (0, 0), "operation={}", operation as char);
        assert_eq!(terminal.grid().scrollback_len(), 0);
    }
}
