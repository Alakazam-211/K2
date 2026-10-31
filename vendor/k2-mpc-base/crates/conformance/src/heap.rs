//! Heap counting for the memory rules (`map.steady_bounded`): engine-neutral, since every
//! engine's memory is heap.
//!
//! A test binary that runs a memory rule installs [`Counting`] as its global allocator:
//!
//! ```ignore
//! #[global_allocator]
//! static A: k2_mpc_conformance::heap::Counting = k2_mpc_conformance::heap::Counting;
//! ```
//!
//! The count is **per thread** (bytes allocated minus bytes freed by this thread), so tests
//! running in parallel don't disturb each other: a rule measures what its own replicas hold.
//! Without the allocator, [`thread_live`] is `None` and a memory rule fails, saying so.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    // const-initialised and without a destructor: reading it never allocates
    static LIVE: Cell<isize> = const { Cell::new(0) };
}

static INSTALLED: AtomicBool = AtomicBool::new(false);

fn add(n: isize) {
    let _ = LIVE.try_with(|c| c.set(c.get() + n));
}

/// The system allocator, counting each thread's live bytes.
pub struct Counting;

// SAFETY: every call is forwarded to `System` unchanged; the bookkeeping only touches a
// const-initialised thread-local `Cell`, which never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            INSTALLED.store(true, Ordering::Relaxed);
            add(l.size() as isize);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(l);
        if !p.is_null() {
            INSTALLED.store(true, Ordering::Relaxed);
            add(l.size() as isize);
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        add(-(l.size() as isize));
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = System.realloc(p, l, n);
        if !q.is_null() {
            add(n as isize - l.size() as isize);
        }
        q
    }
}

/// Bytes this thread holds now (allocated minus freed), when [`Counting`] is the global
/// allocator; `None` otherwise.
pub fn thread_live() -> Option<isize> {
    INSTALLED
        .load(Ordering::Relaxed)
        .then(|| LIVE.try_with(Cell::get).unwrap_or(0))
}

/// The heap a value keeps: this thread's live bytes after `make` returns, minus before
/// (what `make` allocated and didn't free, its result included). Panics without
/// [`Counting`].
pub fn retained<T>(make: impl FnOnce() -> T) -> (T, isize) {
    let before = thread_live().expect(
        "memory rules need the counting allocator: install k2_mpc_conformance::heap::Counting \
         as the test binary's #[global_allocator]",
    );
    let v = make();
    let after = thread_live().expect("the counting allocator");
    (v, after - before)
}
