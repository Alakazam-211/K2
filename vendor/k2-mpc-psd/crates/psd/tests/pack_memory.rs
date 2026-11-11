//! The pack's memory, measured with a counting allocator: this test binary holds only this test,
//! so the peak it reads is the index's. A generated PSB of several GiB goes through the whole
//! index (walk, hashes, tree, draft) and never on disk; its pixels are hashed as they pass and
//! never held.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_psd::index_psd;

#[path = "support/big.rs"]
mod big;
use big::big_psb;

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

#[test]
fn a_multi_gib_psb_indexes_in_a_small_fixed_heap() {
    let (layers, channels) = (2u16, 3u16);
    let (mut src, total) = big_psb(layers, channels, GIB + 7);
    assert!(total > 6 * GIB, "the generated file is multi-GiB");
    let limits = IndexLimits::new(100_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let before = CURRENT.load(Relaxed);
    PEAK.store(before, Relaxed);
    let draft = index_psd(&mut src, &limits).expect("check: the generated PSB indexes");
    let peak = PEAK.load(Relaxed) - before;
    assert_eq!(draft.source, "psb-2");
    assert_eq!(draft.nodes.len(), usize::from(layers));
    assert!(
        draft.nodes.iter().all(|n| n.content.is_some()),
        "every layer's content was hashed"
    );
    assert_eq!(
        draft.nodes[0].content, draft.nodes[1].content,
        "two layers of the same zeros hash alike"
    );
    eprintln!("generated PSB: file_bytes={total} layers={layers} peak_heap_bytes={peak}");
    assert!(
        peak < 2 * 1024 * 1024,
        "the index's heap peak was {peak} bytes for a {total}-byte file"
    );
}
