//! Hostile input. The producer is another process with the ring's file open
//! read-write, so every word of the file may be corrupt, torn or chosen to
//! hurt: the consumer must never panic, never fault, and never hand out a
//! frame that doesn't fit its own layout. The producer gets the same
//! treatment from a corrupt file, and the C surface from bad arguments: an
//! error code, never a panic or an abort.
//!
//! The cases first, then a seeded random test (not a coverage-guided
//! fuzzer: it runs the same inputs every time). `KFR_HOSTILE_ITERS` sets
//! its rounds (default 20,000) and `KFR_HOSTILE_SEED` its seed.

use crate::ffi::*;
use crate::layout::*;
use crate::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// xorshift64*: deterministic, and no dependency.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }

    /// Half the time a value from `INTERESTING`, else any 64 bits.
    fn pick(&mut self) -> u64 {
        if self.below(2) == 0 {
            INTERESTING[self.below(INTERESTING.len())]
        } else {
            self.next_u64()
        }
    }
}

/// Values at the edges: sizes of the box and past it, the 56-bit frame
/// number's end, sign bits, all ones.
const INTERESTING: &[u64] = &[
    0,
    1,
    2,
    3,
    15,
    16,
    17,
    47,
    48,
    63,
    64,
    65,
    90,
    180,
    270,
    720,
    1280,
    4096,
    4097,
    65_535,
    691_200,
    1 << 31,
    (1 << 32) - 1,
    1 << 32,
    (1 << 56) - 1,
    1 << 56,
    1 << 63,
    u64::MAX,
];

fn rounds() -> u64 {
    match std::env::var("KFR_HOSTILE_ITERS") {
        Ok(v) => v.parse().expect("KFR_HOSTILE_ITERS is a number"),
        Err(_) => 20_000,
    }
}

fn seed() -> u64 {
    match std::env::var("KFR_HOSTILE_SEED") {
        Ok(v) => v.parse().expect("KFR_HOSTILE_SEED is a number"),
        Err(_) => 0x9e37_79b9_7f4a_7c15,
    }
}

fn temp_ring(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("kfr-hostile-{name}-{}-{n}", std::process::id()))
}

fn no_limit(w: u32, h: u32, slots: u32) -> Layout {
    let mut l = Layout::new(w, h);
    l.slots = slots;
    l.max_fps = 0;
    l
}

/// What a slot claims, written as the producer would (sequence even,
/// `H_LATEST` pointing at it).
#[derive(Clone, Copy)]
struct Claim {
    slot: u64,
    frame_no: u64,
    width: u32,
    height: u32,
    rotation: u32,
    color: u32,
    len: u64,
}

impl Claim {
    fn new(frame_no: u64, width: u32, height: u32) -> Claim {
        Claim {
            slot: 0,
            frame_no,
            width,
            height,
            rotation: 0,
            color: 0,
            len: nv12_len(width, height) as u64,
        }
    }

    fn write(&self, c: &Consumer) {
        let m = c.raw_map();
        let l = c.layout();
        let base = HEADER + (self.slot as usize % l.slots as usize) * l.slot_stride();
        m.u64(base + SL_SEQ).store(2, Ordering::Relaxed);
        m.u64(base + SL_FRAME_NO)
            .store(self.frame_no, Ordering::Relaxed);
        m.u32(base + SL_WIDTH).store(self.width, Ordering::Relaxed);
        m.u32(base + SL_HEIGHT).store(self.height, Ordering::Relaxed);
        m.u32(base + SL_ROTATION)
            .store(self.rotation, Ordering::Relaxed);
        m.u32(base + SL_COLOR).store(self.color, Ordering::Relaxed);
        m.u64(base + SL_TIMESTAMP).store(7, Ordering::Relaxed);
        m.u64(base + SL_LEN).store(self.len, Ordering::Relaxed);
        m.u64(H_LATEST)
            .store(self.frame_no << 8 | self.slot, Ordering::Release);
    }
}

/// A frame the consumer handed out must fit its layout, and its bytes must
/// be exactly its length.
fn check_frame(c: &Consumer, info: &FrameInfo, buf: &[u8]) {
    let l = c.layout();
    assert!(info.width >= 1 && info.height >= 1, "{info:?}");
    assert!(l.fits(info.width, info.height), "{info:?} outside {l:?}");
    assert!(matches!(info.rotation, 0 | 90 | 180 | 270), "{info:?}");
    assert!(
        (1..=MAX_FRAME_NO).contains(&info.frame_no),
        "{info:?}"
    );
    assert_eq!(buf.len(), info.len(), "{info:?}");
    assert!(info.len() <= l.slot_bytes());
    assert!(info.uv_offset() <= buf.len());
    assert_eq!(
        info.uv_offset() + info.uv_stride() * (info.height as usize).div_ceil(2),
        buf.len()
    );
}

#[test]
fn huge_sizes_are_refused_without_overflow() {
    let path = temp_ring("huge");
    let mut c = Consumer::create(&path, Layout::hd720()).expect("create");
    let mut buf = Vec::new();
    for (w, h, len) in [
        (u32::MAX, u32::MAX, 1000),
        (u32::MAX, u32::MAX, u64::MAX),
        // Sizes whose NV12 length wraps a 64-bit sum to a small number.
        (4_294_929_473, 2_863_336_746, 1_089_044),
        (1 << 31, 1 << 31, 0),
        (4097, 2, nv12_len(4097, 2) as u64),
    ] {
        let mut claim = Claim::new(1, w, h);
        claim.len = len;
        claim.write(&c);
        assert_eq!(c.try_read(&mut buf), None, "{w}x{h} len {len}");
        assert_eq!(c.read_latest(&mut buf), None, "{w}x{h} len {len}");
    }
    assert!(c.stats().rejected >= 1);
    // The size maths saturates instead of overflowing.
    assert_eq!(
        nv12_len(u32::MAX, u32::MAX),
        usize::try_from(u64::MAX).unwrap_or(usize::MAX)
    );
    let mut big = Layout::new(u32::MAX, u32::MAX);
    big.slots = u32::MAX;
    assert_eq!(big.file_len(), usize::MAX);
    assert!(big.validate().is_err());
}

#[test]
fn frames_outside_the_box_or_with_bad_rotation_or_colour_are_refused() {
    let path = temp_ring("box");
    let mut c = Consumer::create(&path, Layout::hd720()).expect("create");
    let mut buf = Vec::new();
    let mut n = 0;
    let mut refused = |claim: Claim| {
        n += 1;
        let mut claim = claim;
        claim.frame_no = n;
        claim.write(&c);
        assert_eq!(c.try_read(&mut buf), None, "frame {n}");
    };
    // Tall and thin: a pixel count that fits, a shape that doesn't.
    let mut thin = Claim::new(0, 1, 691_200);
    thin.rotation = 37;
    refused(thin);
    refused(Claim::new(0, 1, 691_200));
    refused(Claim::new(0, 1282, 720));
    refused(Claim::new(0, 1280, 722));
    refused(Claim::new(0, 0, 720));
    refused(Claim::new(0, 1280, 0));
    for rotation in [1, 37, 89, 91, 360, u32::MAX] {
        let mut claim = Claim::new(0, 64, 48);
        claim.rotation = rotation;
        refused(claim);
    }
    for color in [3, 7, u32::MAX] {
        let mut claim = Claim::new(0, 64, 48);
        claim.color = color;
        refused(claim);
    }
    // A length that disagrees with the size, either way.
    for len in [0, nv12_len(64, 48) as u64 - 1, nv12_len(64, 48) as u64 + 1] {
        let mut claim = Claim::new(0, 64, 48);
        claim.len = len;
        refused(claim);
    }
    // A slot past the layout's last (it has 3).
    c.raw_map()
        .u64(H_LATEST)
        .store(999 << 8 | 3, Ordering::Release);
    assert_eq!(c.try_read(&mut buf), None);
    // Each bad frame counted once, however often it is looked at.
    let before = c.stats().rejected;
    assert_eq!(c.try_read(&mut buf), None);
    assert_eq!(c.stats().rejected, before);

    // Inside the box either way round, and every quarter turn: read.
    for (i, (w, h, rotation)) in [(1280, 720, 0), (720, 1280, 90), (2, 2, 180), (1, 1, 270)]
        .into_iter()
        .enumerate()
    {
        let mut claim = Claim::new(10_000 + i as u64, w, h);
        claim.rotation = rotation;
        claim.write(&c);
        let info = c.try_read(&mut buf).expect("a frame that fits");
        check_frame(&c, &info, &buf);
        assert_eq!((info.width, info.height, info.rotation), (w, h, rotation));
    }
}

#[test]
fn a_stray_frame_number_does_not_freeze_the_ring() {
    let path = temp_ring("frame-no");
    let mut c = Consumer::create(&path, no_limit(64, 48, 3)).expect("create");
    let mut buf = Vec::new();
    let last = Claim::new(MAX_FRAME_NO, 64, 48);
    last.write(&c);
    let info = c.try_read(&mut buf).expect("the last number is still a frame");
    assert_eq!(info.frame_no, MAX_FRAME_NO);
    // The next producer starts again at 1, and every frame comes through.
    let mut p = Producer::attach(&path).expect("attach");
    p.start();
    let (y, uv) = (vec![1u8; 64 * 48], vec![2u8; 64 * 24]);
    for want in 1..=5 {
        let pushed = p.push(&Nv12Frame::new(&y, 64, &uv, 64, 64, 48)).expect("push");
        assert!(
            matches!(pushed, Pushed::Written { frame_no, .. } if frame_no == want),
            "{pushed:?}"
        );
        let info = c.try_read(&mut buf).expect("a new frame");
        assert_eq!(info.frame_no, want);
        check_frame(&c, &info, &buf);
    }
    // A number past 56 bits in the file: the producer still writes frames
    // the consumer reads.
    c.raw_map().u64(H_LATEST).store(u64::MAX, Ordering::Release);
    drop(p);
    let mut p = Producer::attach(&path).expect("attach again");
    p.start();
    let pushed = p.push(&Nv12Frame::new(&y, 64, &uv, 64, 64, 48)).expect("push");
    assert!(matches!(pushed, Pushed::Written { frame_no: 1, .. }), "{pushed:?}");
    let info = c.try_read(&mut buf).expect("read after a wild number");
    assert_eq!(info.frame_no, 1);
}

#[test]
fn a_shrunk_file_reads_as_nothing_instead_of_faulting() {
    let path = temp_ring("shrunk");
    let layout = Layout::hd720();
    let mut c = Consumer::create(&path, layout).expect("create");
    let mut p = Producer::attach(&path).expect("attach");
    p.start();
    let (y, uv) = (vec![1u8; 64 * 48], vec![2u8; 64 * 24]);
    p.push(&Nv12Frame::new(&y, 64, &uv, 64, 64, 48)).expect("push");
    p.push(&Nv12Frame::new(&y, 64, &uv, 64, 64, 48))
        .expect("push again");
    let full = layout.file_len() as u64;
    let mut buf = Vec::new();
    for len in [0, 100, HEADER as u64, 4096, full / 2, full - 1] {
        c.file().set_len(len).expect("shrink");
        assert_eq!(c.try_read(&mut buf), None, "{len} bytes");
        assert_eq!(c.read_latest(&mut buf), None, "{len} bytes");
        assert!(
            matches!(c.producer(), Err(RingError::Mismatch(_))),
            "{len} bytes"
        );
        let _ = c.stats();
        c.touch();
        c.set_wanted(true);
        c.request_stop();
        c.set_max_fps(10).expect("fps");
        c.file().set_len(full).expect("restore");
    }
    // Shrunk when the consumer goes: its drop doesn't fault either.
    c.file().set_len(100).expect("shrink");
    drop(c);
    drop(p);
}

#[test]
fn bad_arguments_to_the_c_surface_return_an_error() {
    let path = temp_ring("ffi");
    let _c = Consumer::create(&path, no_limit(64, 48, 3)).expect("create");
    let cpath = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).expect("c path");
    let mut err = 0;
    // SAFETY: a NUL-terminated path and a valid error slot.
    let p = unsafe { kfr_producer_attach(cpath.as_ptr(), &mut err) };
    assert!(!p.is_null(), "attach: {err}");
    // SAFETY: a live producer.
    assert_eq!(unsafe { kfr_producer_start(p) }, KFR_OK);
    let (y, uv) = ([0u8; 8], [0u8; 8]);
    // (y_len, y_stride, uv_len, uv_stride, width, height)
    let cases: &[(usize, usize, usize, usize, u32, u32)] = &[
        // A Y stride whose (rows - 1) multiple overflows.
        (1, 1 << 63, 8, 2, 1, 3),
        (8, usize::MAX, 8, 2, 1, 3),
        (8, usize::MAX / 2 + 1, 8, 2, 1, 3),
        // A UV stride whose (rows - 1) multiple overflows.
        (5, 1, 8, 1 << 63, 1, 5),
        (5, 1, 8, usize::MAX, 1, 5),
        // Planes shorter than their sizes say.
        (8, 4, 8, 4, 4, 4),
        (8, 2, 2, 2, 2, 3),
        // Sizes out of range.
        (8, 1, 8, 2, 0, 1),
        (8, 1, 8, 2, 1, 0),
        (8, 1, 8, 2, u32::MAX, 1),
        (8, 1, 8, 2, 1, u32::MAX),
        // Strides narrower than the width.
        (8, 1, 8, 2, 2, 1),
        (8, 2, 8, 1, 2, 1),
    ];
    for &(y_len, y_stride, uv_len, uv_stride, w, h) in cases {
        // SAFETY: a live producer; `y` and `uv` hold at least the lengths
        // given.
        let rc = unsafe {
            kfr_producer_push_nv12(
                p,
                y.as_ptr(),
                y_len,
                y_stride,
                uv.as_ptr(),
                uv_len,
                uv_stride,
                w,
                h,
                0,
                0,
                0,
            )
        };
        assert_eq!(
            rc, KFR_ERR_INVALID,
            "{y_len} {y_stride} {uv_len} {uv_stride} {w}x{h}"
        );
    }
    // A length no buffer can have is refused before it becomes a slice.
    // SAFETY: refused before the pointers are read.
    let rc = unsafe {
        kfr_producer_push_nv12(
            p,
            y.as_ptr(),
            usize::MAX,
            1,
            uv.as_ptr(),
            4,
            2,
            1,
            1,
            0,
            0,
            0,
        )
    };
    assert_eq!(rc, KFR_ERR_INVALID);
    // Still a working producer.
    // SAFETY: a live producer, planes of the lengths given.
    let rc = unsafe {
        kfr_producer_push_nv12(p, y.as_ptr(), 8, 2, uv.as_ptr(), 8, 2, 2, 2, 90, 2, 0)
    };
    assert_eq!(rc, KFR_OK);
    // SAFETY: finished once.
    unsafe { kfr_producer_finish(p, 1) };
}

#[test]
fn a_planted_temporary_file_or_link_is_never_written() {
    use std::os::unix::fs::symlink;
    let dir = temp_ring("plant");
    std::fs::create_dir(&dir).expect("dir");
    let victim = dir.join("victim");
    std::fs::write(&victim, b"keep me").expect("victim");
    let path = dir.join("screen.ring");
    // The consumer's temporary name carries the clock, so it can't be known
    // ahead. Plant the old fixed name and a guess at the new shape anyway:
    // links to a file the app can write.
    for name in [
        format!("screen.ring.new-{}", std::process::id()),
        format!("screen.ring.new-{}-0-0", std::process::id()),
    ] {
        symlink(&victim, dir.join(name)).expect("plant");
    }
    let c = Consumer::create(&path, no_limit(64, 48, 2)).expect("create");
    assert_eq!(std::fs::read(&victim).expect("victim"), b"keep me");
    assert!(std::fs::symlink_metadata(&path)
        .expect("ring")
        .file_type()
        .is_file());
    drop(c);
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// Random, corrupt and hostile words in the header and the slots, torn
/// slots, random frame bytes, a real producer now and then, and a shrunk
/// file: the consumer never panics or faults, and every frame it hands out
/// fits its layout.
#[test]
fn the_consumer_survives_random_hostile_rings() {
    let mut rng = Rng(seed());
    let layouts = [
        no_limit(64, 48, 3),
        no_limit(16, 16, 2),
        no_limit(64, 32, 8),
        no_limit(48, 64, 4),
    ];
    let mut handed_out = 0u64;
    let mut rings: Vec<(PathBuf, Consumer)> = layouts
        .iter()
        .map(|l| {
            let path = temp_ring("random");
            let c = Consumer::create(&path, *l).expect("create");
            (path, c)
        })
        .collect();
    let mut buf = Vec::new();
    for round in 0..rounds() {
        let (path, c) = &mut rings[rng.below(layouts.len())];
        let l = c.layout();
        let stride = l.slot_stride();
        let m = c.raw_map();
        for _ in 0..1 + rng.below(4) {
            match rng.below(8) {
                0 => {
                    let off = rng.below(HEADER / 8) * 8;
                    m.u64(off).store(rng.pick(), Ordering::Relaxed);
                }
                1 => {
                    let off = rng.below(HEADER / 4) * 4;
                    m.u32(off).store(rng.pick() as u32, Ordering::Relaxed);
                }
                2 => {
                    let base = HEADER + rng.below(l.slots as usize) * stride;
                    let off = rng.below(SLOT_HEADER / 4) * 4;
                    m.u32(base + off)
                        .store(rng.pick() as u32, Ordering::Relaxed);
                }
                3 => {
                    let base = HEADER + rng.below(l.slots as usize) * stride;
                    let off = rng.below(SLOT_HEADER / 8) * 8;
                    m.u64(base + off).store(rng.pick(), Ordering::Relaxed);
                }
                4 | 5 => {
                    // A whole lie: a slot that claims anything, published.
                    let claim = Claim {
                        slot: if rng.below(4) == 0 {
                            rng.pick() & 0xff
                        } else {
                            rng.below(l.slots as usize) as u64
                        },
                        frame_no: if rng.below(2) == 0 {
                            round + 1
                        } else {
                            rng.pick() & MAX_FRAME_NO
                        },
                        width: if rng.below(2) == 0 {
                            1 + rng.below(l.max_width.max(l.max_height) as usize + 2) as u32
                        } else {
                            rng.pick() as u32
                        },
                        height: if rng.below(2) == 0 {
                            1 + rng.below(l.max_width.max(l.max_height) as usize + 2) as u32
                        } else {
                            rng.pick() as u32
                        },
                        rotation: [0, 90, 180, 270, 45, rng.pick() as u32][rng.below(6)],
                        color: rng.below(5) as u32,
                        len: 0,
                    };
                    let len = match rng.below(4) {
                        0 => rng.pick(),
                        _ => nv12_len(claim.width, claim.height) as u64,
                    };
                    let claim = Claim { len, ..claim };
                    claim.write(c);
                    if rng.below(8) == 0 {
                        // Torn: the sequence odd.
                        let base = HEADER + (claim.slot as usize % l.slots as usize) * stride;
                        m.u64(base + SL_SEQ).fetch_or(1, Ordering::Relaxed);
                    }
                }
                6 => {
                    // Random frame bytes in a random slot.
                    let base = HEADER + rng.below(l.slots as usize) * stride + SLOT_HEADER;
                    let at = rng.below(l.slot_bytes());
                    let n = rng.below(l.slot_bytes() - at + 1);
                    let byte = rng.next_u64() as u8;
                    // SAFETY: `n` bytes of the slot's frame area, which this
                    // thread alone uses.
                    unsafe { std::ptr::write_bytes(m.bytes(base + at, n), byte, n) };
                }
                _ => {
                    // A real producer, if the file lets one attach.
                    if let Ok(mut p) = Producer::attach(&*path) {
                        p.start();
                        let w = 1 + rng.below(80) as u32;
                        let h = 1 + rng.below(80) as u32;
                        let (y, uv) = (
                            vec![rng.next_u64() as u8; (w * h) as usize],
                            vec![128u8; nv12_len(w, h) - (w * h) as usize],
                        );
                        let uv_stride = 2 * (w as usize).div_ceil(2);
                        let mut f = Nv12Frame::new(&y, w as usize, &uv, uv_stride, w, h);
                        f.rotation = [0, 90, 180, 270][rng.below(4)];
                        // Valid frames go in; refusals are fine too (a
                        // corrupt rate, a consumer marked gone).
                        let _ = p.push(&f);
                    }
                }
            }
        }
        // Sometimes the file shrinks under the reader.
        let shrunk = rng.below(16) == 0;
        let full = l.file_len() as u64;
        if shrunk {
            // Shorter than the ring, whatever its size (a small ring is under 4 KB).
            let len = [0, 100, 4096, full / 2, full - 1][rng.below(5)].min(full - 1);
            c.file().set_len(len).expect("shrink");
        }
        let a = c.try_read(&mut buf);
        if let Some(info) = &a {
            check_frame(c, info, &buf);
            handed_out += 1;
            // Read once: the same frame isn't new again.
            if c.try_read(&mut buf).is_some_and(|again| again.frame_no == info.frame_no) {
                panic!("frame {} handed out twice", info.frame_no);
            }
        }
        if let Some(info) = c.read_latest(&mut buf) {
            check_frame(c, &info, &buf);
        }
        let state = c.producer();
        assert_eq!(shrunk, state.is_err(), "{state:?}");
        let _ = c.stats();
        assert_eq!(c.layout(), l, "the layout is the consumer's own");
        c.set_wanted(rng.below(2) == 0);
        c.touch();
        if shrunk {
            assert!(a.is_none(), "a frame from a shrunk file");
            c.file().set_len(full).expect("restore");
        }
    }
    eprintln!(
        "hostile rings: {} rounds, {handed_out} frames handed out",
        rounds()
    );
    assert!(handed_out > 0, "no round produced a valid frame");
}

/// Producers attach to random and corrupt files: an error or a working
/// producer, never a panic.
#[test]
fn the_producer_survives_random_files() {
    let mut rng = Rng(seed() ^ 0x5bd1_e995);
    let layout = no_limit(64, 48, 3);
    let len = layout.file_len();
    for round in 0..rounds() / 20 {
        let path = temp_ring("attach");
        let c = Consumer::create(&path, layout).expect("create");
        let m = c.raw_map();
        match rng.below(3) {
            0 => {
                for _ in 0..1 + rng.below(6) {
                    let off = rng.below(HEADER / 8) * 8;
                    m.u64(off).store(rng.pick(), Ordering::Relaxed);
                }
            }
            1 => {
                let off = rng.below(HEADER / 4) * 4;
                m.u32(off).store(rng.pick() as u32, Ordering::Relaxed);
            }
            _ => {
                let lens = [0, 1, HEADER - 1, HEADER, len - 1, len + 1, 4096];
                c.file()
                    .set_len(lens[rng.below(lens.len())] as u64)
                    .expect("resize");
            }
        }
        if let Ok(mut p) = Producer::attach(&path) {
            p.start();
            let (y, uv) = (vec![9u8; 64 * 48], vec![9u8; 64 * 24]);
            let mut f = Nv12Frame::new(&y, 64, &uv, 64, 64, 48);
            f.timestamp_ns = round + 1;
            let _ = p.push(&f);
            let _ = (p.wants_frames(), p.stop_requested(), p.layout());
        }
        c.file().set_len(len as u64).expect("restore");
        drop(c);
    }
}
