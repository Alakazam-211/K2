//! Hostile streams refused, typed, within a small heap: a decompression bomb, a page declaring a
//! huge size, a text declaring more units than its bytes hold. The heap is measured with a
//! counting allocator: every row holds the lock from its first line to its last, so no other
//! row allocates or frees while it measures; each prints `peak_heap_bytes`.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

use k2_mpc_dwg::bits::Bits;
use k2_mpc_dwg::lz77::decompress;
use k2_mpc_dwg::Limits;
use support::{coded, literal, pack, refused};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LOCK: Mutex<()> = Mutex::new(());

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

const MIB: usize = 1 << 20;

/// Held for a whole row: the rows run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs `f`, and its heap peak above what was live before it (inside [`serial`]).
fn measured<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = CURRENT.load(Relaxed);
    PEAK.store(before, Relaxed);
    let out = f();
    let peak = PEAK.load(Relaxed).saturating_sub(before);
    (out, peak)
}

#[test]
pub fn bomb_within_a_small_heap() {
    let _one = serial();
    // a 131 KB stream whose one match asks for 33 MB, on a page declaring 0x7400 bytes
    let mut src = literal(b"ABCD");
    src.push(0x20);
    src.extend(std::iter::repeat_n(0u8, 131_072));
    src.extend_from_slice(&[0x01, 0x0C, 0x00, 0x11]);
    let (got, peak) = measured(|| decompress(&src, 0x7400, &Limits::default()));
    println!("bomb_within_a_small_heap: peak_heap_bytes {peak}");
    assert!(peak < MIB, "the bomb grew the heap to {peak} bytes");
    refused(got, "DWG-E005", "a match past the declared size");
}

#[test]
pub fn declared_size_bounded() {
    let _one = serial();
    let (got, peak) = measured(|| decompress(b"\x11", 64 * MIB, &Limits::default()));
    println!("declared_size_bounded: peak_heap_bytes {peak}");
    assert!(peak < MIB, "a declared size allocated {peak} bytes");
    refused(got, "DWG-E005", "a declared size over the page limit");
}

#[test]
pub fn text_length_bounded() {
    let _one = serial();
    // a TU declaring 65,535 units, with no bytes after it
    let buf = pack(&coded(0b00, &[0xFF, 0xFF]));
    let (got, peak) = measured(|| Bits::new(&buf).tu());
    println!("text_length_bounded: peak_heap_bytes {peak}");
    assert!(
        peak < 16 * 1024,
        "a text's declared length allocated {peak} bytes"
    );
    refused(got, "DWG-E002", "a text longer than its bytes");
}

#[test]
pub fn section_page_count_bounded() {
    let _one = serial();
    // a section map claiming four million pages for one section
    let mut spec = support::container::standard();
    spec.sections[0].pages_claimed = Some(4 << 20);
    let bytes = support::container::build(&spec).bytes;
    let (got, peak) = measured(|| k2_mpc_dwg::read_container(&bytes, &Limits::default()));
    println!("section_page_count_bounded: peak_heap_bytes {peak}");
    assert!(peak < MIB, "a claimed page count allocated {peak} bytes");
    refused(got, "DWG-E006", "more pages than the section map holds");
}

#[test]
pub fn section_size_bounded() {
    let _one = serial();
    // a section claiming 8 MiB, against a limit of 1 MiB
    let mut spec = support::container::standard();
    spec.sections[1].size_claimed = Some(8 * MIB as u64);
    let bytes = support::container::build(&spec).bytes;
    let limits = Limits {
        max_section: MIB as u64,
        ..Limits::default()
    };
    let c = k2_mpc_dwg::read_container(&bytes, &limits)
        .unwrap_or_else(|e| panic!("the container refused: {}", e.summary()));
    let (got, peak) = measured(|| k2_mpc_dwg::read_section(&bytes, &c, "AcDb:Classes", &limits));
    println!("section_size_bounded: peak_heap_bytes {peak}");
    assert!(peak < MIB, "a claimed section size allocated {peak} bytes");
    refused(got, "DWG-E005", "a section over the section limit");
}

#[test]
pub fn object_map_entries_bounded() {
    let _one = serial();
    // an object map of 300,000 entries (handle +1, location +0), in blocks of 32,766, against a
    // limit of 1,000
    let mut map = Vec::new();
    let mut left = 300_000usize;
    while left > 0 {
        let n = left.min(32_766);
        left -= n;
        let mut block = ((2 * n + 2) as u16).to_be_bytes().to_vec();
        for _ in 0..n {
            block.extend_from_slice(&[0x01, 0x00]);
        }
        let crc = k2_mpc_dwg::crc::crc16(k2_mpc_dwg::crc::CRC16_SEED, &block);
        block.extend_from_slice(&crc.to_be_bytes());
        map.extend(block);
    }
    map.extend_from_slice(&[0x00, 0x02]);
    let limits = Limits {
        max_objects: 1_000,
        ..Limits::default()
    };
    let (got, peak) = measured(|| k2_mpc_dwg::objects::object_map(&map, &limits));
    println!("object_map_entries_bounded: peak_heap_bytes {peak}");
    assert!(
        peak < 256 * 1024,
        "an object map's entries grew the heap to {peak} bytes"
    );
    refused(got, "DWG-E005", "more entries than the object limit");
}

/// A light-weight polyline claiming four million points in a few bytes: refused before its
/// points are allocated.
#[test]
pub fn polyline_count_bounded() {
    use k2_mpc_dwg::entities::entity;
    use k2_mpc_dwg::frame::Ver;
    use support::object::{bd, bl, bs, ent, Common};
    let _one = serial();
    let v = Ver::R2018;
    let mut f = bs(0);
    f.extend(bl(4 << 20));
    f.extend(bd(0.0));
    let obj = ent(v, 0x4D, 0x40, &Common::on(0x10), f, vec![], vec![]);
    let (r, peak) = measured(|| entity(&obj.0, obj.1, v, &[]).map(|_| ()));
    println!("peak_heap_bytes {peak} (a polyline's count)");
    refused(r, "DWG-E003", "more points than the data holds");
    assert!(peak < MIB, "peak {peak} bytes: the points were allocated");
}

/// A hatch claiming a million boundary paths in a few bytes: refused before they are
/// allocated.
#[test]
pub fn hatch_count_bounded() {
    use k2_mpc_dwg::entities::entity;
    use k2_mpc_dwg::frame::Ver;
    use support::object::{b, bd, bd3, bl, ent, Common};
    let _one = serial();
    let v = Ver::R2018;
    let mut f = [bl(0), bl(0), bd(0.0), bd(0.0), bl(0), bd(0.0), bl(0)].concat();
    f.extend(
        [
            bd(0.0),
            bd3([0.0, 0.0, 1.0]),
            b(true),
            b(false),
            bl(1 << 20),
        ]
        .concat(),
    );
    let obj = ent(v, 0x4E, 0x41, &Common::on(0x10), f, vec![], vec![]);
    let (r, peak) = measured(|| entity(&obj.0, obj.1, v, &[]).map(|_| ()));
    println!("peak_heap_bytes {peak} (a hatch's path count)");
    refused(r, "DWG-E003", "more paths than the data holds");
    assert!(peak < MIB, "peak {peak} bytes: the paths were allocated");
}
