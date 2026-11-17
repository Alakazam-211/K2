//! zstd streams crafted to decode far past the reader's raw cap, read under a 1 MiB cap with a
//! counting allocator. The rows here run one at a time (a lock), so the peak each reads is its
//! own read's. Every stream is built by code when the row runs (`support::zstd`), under 128 KiB.

mod support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;

use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{read_blend, ReadError, ReadLimits};
use support::zstd::{frame, long_block_head, Block, BLOCK_MAX, MATCH_MAX};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

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

/// The raw cap the rows read under.
const CAP: u64 = 1 << 20;
/// The heap a read may peak at: a small multiple of the cap.
const HEAP_MAX: usize = 8 * CAP as usize;
/// A crafted stream's most bytes.
const INPUT_MAX: usize = 128 << 10;

fn limits() -> ReadLimits {
    let d = ReadLimits::default();
    ReadLimits::new(
        CAP,
        d.max_blocks,
        d.max_retained_bytes,
        d.retain_block_max,
        d.max_sdna_bytes,
        d.max_window_bytes,
    )
}

/// `f`'s result and the heap's peak over it, above what was held before.
fn peak_of<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = CURRENT.load(Relaxed);
    PEAK.store(before, Relaxed);
    let out = f();
    (out, PEAK.load(Relaxed).saturating_sub(before))
}

/// One compressed block whose sequences ask for over 512 MiB, behind the head of a file whose
/// first block is 2 GiB long: refused, typed, with the heap under a small multiple of the cap,
/// by the streaming read and by the whole-file decode. The block is over the format's own
/// bound on one block's output (128 KiB), so the decoder refuses it before any of it is read.
#[test]
fn one_block_asking_for_half_a_gib_is_refused_in_a_small_heap() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let head = long_block_head();
    let sequences = 4096;
    let bytes = frame(
        17,
        &[
            Block::Raw(&head),
            Block::Repeats {
                byte: 0x55,
                sequences,
            },
        ],
    );
    let asks = u64::from(sequences) * (1 + MATCH_MAX);
    assert!(bytes.len() < INPUT_MAX, "check: {} bytes", bytes.len());
    assert!(asks > 512 * CAP, "check: the block asks for {asks} bytes");
    let (read, peak) = peak_of(|| read_blend(&mut &bytes[..], &limits()));
    eprintln!(
        "one block: input_bytes={} asks_bytes={asks} cap={CAP} read_peak_heap_bytes={peak}",
        bytes.len()
    );
    assert!(
        peak < HEAP_MAX,
        "the read's heap peak was {peak} bytes, over {HEAP_MAX}, for a stream of {} bytes",
        bytes.len()
    );
    match read {
        Err(ReadError::Malformed { what }) => assert_eq!(what, "a zstd frame doesn't decode"),
        other => panic!("the read: {:?}", other.map(|f| f.raw_len)),
    }
    let (whole, peak) = peak_of(|| decompress(&bytes, &limits()));
    eprintln!("one block: decompress_peak_heap_bytes={peak}");
    assert!(
        peak < HEAP_MAX,
        "the whole-file decode's heap peak was {peak} bytes, over {HEAP_MAX}"
    );
    match whole {
        Err(ReadError::Malformed { what }) => assert_eq!(what, "a zstd frame doesn't decode"),
        other => panic!("the whole-file decode: {:?}", other.map(|(_, r)| r.len())),
    }
}

/// Blocks each within the format's bound, 16,384 of them for 2 GiB of output from a stream of
/// 64 KiB: refused by the reader's raw cap (`ReadLimits::max_raw_bytes`), typed, with the heap
/// under a small multiple of the cap, by the streaming read and by the whole-file decode.
#[test]
fn many_blocks_past_the_raw_cap_are_refused_by_it_in_a_small_heap() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let head = long_block_head();
    let n = 16_384;
    let mut blocks = vec![Block::Raw(&head)];
    blocks.extend((0..n).map(|_| Block::Rle(0x55, BLOCK_MAX)));
    let bytes = frame(17, &blocks);
    let total = head.len() as u64 + n * u64::from(BLOCK_MAX);
    assert!(bytes.len() < INPUT_MAX, "check: {} bytes", bytes.len());
    assert!(total > 2048 * CAP, "check: {total} bytes after decoding");
    let (read, peak) = peak_of(|| read_blend(&mut &bytes[..], &limits()));
    eprintln!(
        "many blocks: input_bytes={} output_bytes={total} cap={CAP} read_peak_heap_bytes={peak}",
        bytes.len()
    );
    assert!(
        peak < HEAP_MAX,
        "the read's heap peak was {peak} bytes, over {HEAP_MAX}"
    );
    match read {
        Err(ReadError::Limit { what, limit }) => {
            assert_eq!((what, limit), ("bytes after decompression", CAP));
        }
        other => panic!("the read: {:?}", other.map(|f| f.raw_len)),
    }
    let (whole, peak) = peak_of(|| decompress(&bytes, &limits()));
    eprintln!("many blocks: decompress_peak_heap_bytes={peak}");
    assert!(
        peak < HEAP_MAX,
        "the whole-file decode's heap peak was {peak} bytes, over {HEAP_MAX}"
    );
    match whole {
        Err(ReadError::Limit { what, limit }) => {
            assert_eq!((what, limit), ("bytes after decompression", CAP));
        }
        other => panic!("the whole-file decode: {:?}", other.map(|(_, r)| r.len())),
    }
}

/// The streaming index with its decoded-bytes cap set to 1 GiB refuses the 2 GiB multi-block
/// stream by that cap, typed, with the heap under a small multiple of the 1 MiB reader cap the
/// other rows use (`support::zstd::row_index_cap`).
#[test]
fn the_streaming_index_refuses_a_stream_past_its_cap_in_a_small_heap() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (row, peak) = peak_of(support::zstd::row_index_cap);
    eprintln!("index: index_peak_heap_bytes={peak}");
    if let Err(e) = row {
        panic!("{e}");
    }
    assert!(
        peak < HEAP_MAX,
        "the index's heap peak was {peak} bytes, over {HEAP_MAX}"
    );
}

/// A frame whose window is over the decoder's own default bound and within the reader's limit
/// reads; over the reader's limit, it is refused by that limit
/// (`support::zstd::row_window_over_decoder_default`).
#[test]
fn a_window_within_the_readers_limit_reads() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(e) = support::zstd::row_window_over_decoder_default() {
        panic!("{e}");
    }
}

/// The reader's default limits refuse a stream decoding to just past 4 GiB by their raw cap,
/// typed, with the heap under a small multiple of the 1 MiB cap the other rows use
/// (`support::zstd::row_read_default_cap`); and the streaming index's default limits are the
/// reader's own, so the two can't drift apart.
#[test]
fn the_readers_default_refuses_a_stream_past_4_gib_in_a_small_heap() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (row, peak) = peak_of(support::zstd::row_read_default_cap);
    eprintln!("read default: read_peak_heap_bytes={peak}");
    if let Err(e) = row {
        panic!("{e}");
    }
    assert!(
        peak < HEAP_MAX,
        "the read's heap peak was {peak} bytes, over {HEAP_MAX}"
    );
    assert_eq!(
        k2_mpc_blend::pack::index_read_limits(),
        ReadLimits::default(),
        "the streaming index's default limits are the reader's"
    );
    assert_eq!(k2_mpc_blend::pack::INDEX_MAX_RAW_BYTES, 4 << 30);
}
