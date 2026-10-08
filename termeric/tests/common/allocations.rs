//! A global allocator that counts what one thread allocates while
//! [`measure`] runs, for tests that hold a path to an allocation budget. A
//! test binary installs it itself, so only its own allocations go through
//! it:
//!
//! ```ignore
//! #[global_allocator]
//! static ALLOCATOR: common::allocations::CountingAllocator =
//!     common::allocations::CountingAllocator;
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

pub struct CountingAllocator;

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

/// How many allocations `f` makes on this thread, and how many bytes they
/// ask for, when [`CountingAllocator`] is the binary's global allocator.
pub fn measure(f: impl FnOnce()) -> (usize, usize) {
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
