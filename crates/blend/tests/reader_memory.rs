//! The reader's memory, measured with a counting allocator: this test binary holds only this
//! test, so the peak it reads is the read's. A generated file of over a GiB, never on disk:
//! its large data blocks are digested as they pass, never held.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use k2_mpc_blend::testgen::{BigFile, Body, FileBuilder};
use k2_mpc_blend::{read_blend, ReadLimits};

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

const MIB: u64 = 1 << 20;

#[test]
fn a_gib_file_reads_in_a_small_fixed_heap() {
    let mut f = FileBuilder::classic();
    f.schema.add_struct(
        "ID",
        &[("void", "*next"), ("void", "*prev"), ("char", "name[66]")],
    );
    f.schema
        .add_struct("Thing", &[("ID", "id"), ("float", "loc[3]")]);
    let mut body = Body::new(f.schema.len_of("Thing"), false, 8);
    body.raw(16, b"OBBig");
    f.block(b"OB\0\0", "Thing", 0x1_0000_0000, body.bytes);
    let blocks = 72;
    let mut src = BigFile::new(&f, blocks, 16 * MIB);
    let total = src.total;
    assert!(total > 1 << 30, "the generated file is over a GiB");
    let before = CURRENT.load(Relaxed);
    PEAK.store(before, Relaxed);
    let read =
        read_blend(&mut src, &ReadLimits::default()).expect("check: the generated file reads");
    let peak = PEAK.load(Relaxed) - before;
    assert_eq!(read.raw_len, total, "read the whole file");
    assert_eq!(read.blocks.len() as u64, 1 + blocks + 2);
    assert!(
        read.body(1).is_none() && read.body_digest(1).is_some(),
        "a 16 MiB block is digested"
    );
    eprintln!("generated file: file_bytes={total} blocks={blocks} peak_heap_bytes={peak}");
    assert!(
        peak < 2 * MIB as usize,
        "the read's heap peak was {peak} bytes for a {total}-byte file"
    );
}
