//! The reuse call's memory, measured with a counting allocator (this test binary holds only
//! this test, so the peak it reads is the call's): over a carrier whose data section is
//! megabytes and whose header is kilobytes, the peak during a call is the header and a small
//! scratch, never a copy of the data.

mod live_support;
mod reuse_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use k2_mpc_blend::live::OBJ_PROPS;
use k2_mpc_blend::meshread::MeshReadLimits;
use reuse_support::{carrier, grid_scene, obj_key, set};

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

/// The scratch a call may hold beside its header: per header byte, and fixed.
const PER_HEADER_BYTE: usize = 16;
const FIXED: usize = 64 << 10;

#[test]
fn a_call_holds_its_header_and_a_small_scratch() {
    let bytes = grid_scene(40);
    let (cv, mut c, p) = carrier(&bytes);
    let g1 = obj_key(&c, "G1").expect("G1");
    set(&mut c, OBJ_PROPS, &g1, "xform", "1,2,3,0,0,0,1,1,1");
    let l = MeshReadLimits::default();
    let state = reuse_support::state_of(&c);
    let before = CURRENT.load(Relaxed);
    PEAK.store(before, Relaxed);
    let out = k2_mpc_blend::live::reuse::live_k2scene_of(&state, &cv, &p, &l);
    let peak = PEAK.load(Relaxed) - before;
    let out = out.expect("the call writes the state");
    let (header, data) = (out.header.len(), out.data.len());
    let bound = PER_HEADER_BYTE * header + FIXED;
    eprintln!(
        "reuse call: objects=40 header_bytes={header} data_bytes={data} peak_heap_bytes={peak} \
         bound={bound}"
    );
    assert!(
        header > 0 && out.changed.len() == 1,
        "the call wrote the move: {out:?}"
    );
    assert!(
        data > 4 * bound,
        "the scene's data ({data} bytes) is large enough beside the bound ({bound}) that a copy \
         shows"
    );
    assert!(
        peak <= bound,
        "the call's heap peak was {peak} bytes, over {bound} (header {header}, data {data})"
    );
}
