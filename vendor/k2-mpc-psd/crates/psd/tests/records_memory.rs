//! The walker's memory, measured with a counting allocator: this test binary holds only this test,
//! so the peak it reads is the walk's. A generated PSB of several GiB, never on disk.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use k2_mpc_psd::{walk_records, WalkOptions};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to the system allocator unchanged; the counters are plain atomics.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let now = CURRENT.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(now, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        CURRENT.fetch_sub(l.size(), Relaxed);
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const GIB: u64 = 1 << 30;

#[path = "support/big.rs"]
mod big;
use big::big_psb;

#[test]
fn a_multi_gib_psb_walks_in_a_small_fixed_heap() {
    let cases = [(2u16, 3u16, GIB), (3, 3, 2 * GIB + 7)];
    for (layers, channels, channel_bytes) in cases {
        let (mut src, total) = big_psb(layers, channels, channel_bytes);
        assert!(total > 5 * GIB, "the generated file is multi-GiB");
        let before = CURRENT.load(Relaxed);
        PEAK.store(before, Relaxed);
        let walked = walk_records(&mut src, &WalkOptions::default(), &mut ())
            .expect("check: the generated PSB walks");
        let peak = PEAK.load(Relaxed) - before;
        assert_eq!(walked.layers.len(), layers as usize);
        assert_eq!(walked.consumed, total, "walked the whole file");
        assert!(walked
            .layers
            .iter()
            .all(|l| l.channels.len() == channels as usize));
        eprintln!("generated PSB: file_bytes={total} layers={layers} peak_heap_bytes={peak}");
        assert!(
            peak < 2 * 1024 * 1024,
            "the walk's heap peak was {peak} bytes for a {total}-byte file"
        );
    }
}
