//! A producer writing into a ring that the other process (the app)
//! scribbles on: the input is a layout pick, then a list of writes into the
//! file (through a second open of it, as the app's process would), pushes,
//! shrinks and new producers attaching. Any panic or fault is a finding,
//! and so is a frame the producer says it wrote that isn't whole in the
//! file: `H_LATEST` naming another frame or a slot past the layout, an odd
//! sequence word, or slot words that aren't the frame's.
//!
//! Run from this folder: `cargo +nightly fuzz run producer`.
#![no_main]

use kessel_frame_ring::{nv12_len, Color, Consumer, Layout, Nv12Frame, Producer, Pushed};
use libfuzzer_sys::fuzz_target;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU32, Ordering};

// The file's format (the crate's `layout` module): a 256-byte header, then
// the slots, each a 64-byte header and an NV12 frame of the box, rounded up
// to 64 bytes. Numbers in the machine's byte order.
const HEADER: u64 = 256;
const SLOT_HEADER: u64 = 64;
const H_FLAGS: u64 = 44;
const H_LATEST: u64 = 64;
const F_WANTS: u32 = 1;
const SL_SEQ: u64 = 0;
const SL_FRAME_NO: u64 = 8;
const SL_WIDTH: u64 = 16;
const SL_HEIGHT: u64 = 20;
const SL_LEN: u64 = 40;

/// Sequence words at the top of their range, and around the producer's
/// own limit (2^62).
const TOPS: [u64; 8] = [
    u64::MAX,
    u64::MAX - 1,
    u64::MAX - 2,
    1 << 63,
    (1 << 62) - 1,
    1 << 62,
    (1 << 62) + 1,
    0,
];

/// Reads bytes off the front of the input; zeros once it runs out.
struct Input<'a>(&'a [u8]);

impl Input<'_> {
    fn byte(&mut self) -> u8 {
        let (b, rest) = self.0.split_first().map_or((0, &[][..]), |(b, r)| (*b, r));
        self.0 = rest;
        b
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes([self.byte(), self.byte(), self.byte(), self.byte()])
    }

    fn bytes(&mut self, n: usize) -> &[u8] {
        let n = n.min(self.0.len());
        let (b, rest) = self.0.split_at(n);
        self.0 = rest;
        b
    }
}

fn read_u64(file: &File, off: u64) -> u64 {
    let mut b = [0; 8];
    file.read_exact_at(&mut b, off).expect("read");
    u64::from_ne_bytes(b)
}

fn read_u32(file: &File, off: u64) -> u32 {
    let mut b = [0; 4];
    file.read_exact_at(&mut b, off).expect("read");
    u32::from_ne_bytes(b)
}

fn stride(l: &Layout) -> u64 {
    (SLOT_HEADER + nv12_len(l.max_width, l.max_height) as u64).div_ceil(64) * 64
}

/// The consumer wants frames and has just looked.
fn want(file: &File, c: &Consumer) {
    file.write_all_at(&F_WANTS.to_ne_bytes(), H_FLAGS)
        .expect("write");
    c.touch();
}

/// The frame `p` says it wrote is whole in the file, and the consumer
/// reads it back when the producer's box is its own.
fn check_written(file: &File, c: &mut Consumer, p: &Producer, frame_no: u64, w: u32, h: u32) {
    let pl = p.layout();
    let latest = read_u64(file, H_LATEST);
    assert_eq!(latest >> 8, frame_no, "H_LATEST names the frame");
    let slot = latest & 0xff;
    assert!(slot < u64::from(pl.slots), "slot {slot} in {pl:?}");
    let base = HEADER + slot * stride(&pl);
    let seq = read_u64(file, base + SL_SEQ);
    assert_eq!(seq % 2, 0, "slot {slot}'s sequence {seq:#x} is even");
    assert!(seq <= (1 << 62) + 2, "slot {slot}'s sequence {seq:#x} in range");
    assert_eq!(read_u64(file, base + SL_FRAME_NO), frame_no);
    assert_eq!(read_u32(file, base + SL_WIDTH), w);
    assert_eq!(read_u32(file, base + SL_HEIGHT), h);
    assert_eq!(read_u64(file, base + SL_LEN), nv12_len(w, h) as u64);
    let fits = (w <= pl.max_width && h <= pl.max_height) || (w <= pl.max_height && h <= pl.max_width);
    assert!(fits, "{w}x{h} outside {pl:?}");
    let l = c.layout();
    if (pl.max_width, pl.max_height, pl.slots) == (l.max_width, l.max_height, l.slots) {
        let mut buf = Vec::new();
        let info = c.read_latest(&mut buf).expect("the frame just written");
        assert_eq!((info.frame_no, info.width, info.height), (frame_no, w, h));
        assert_eq!(buf.len(), nv12_len(w, h));
    }
}

fuzz_target!(|data: &[u8]| {
    static N: AtomicU32 = AtomicU32::new(0);
    let mut input = Input(data);
    let mut layout = match input.byte() % 3 {
        0 => Layout::new(64, 48),
        1 => Layout::new(16, 16),
        _ => Layout::new(48, 32),
    };
    layout.slots = 2 + u32::from(input.byte() % 7);
    layout.max_fps = 0;
    let path = std::env::temp_dir().join(format!(
        "kfr-fuzz-producer-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let mut c = Consumer::create(&path, layout).expect("create");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .expect("open the ring as the other process");
    let full = layout.file_len() as u64;
    let mut p = Producer::attach(&path).ok();
    if let Some(p) = &p {
        p.start();
    }
    let mut ts = 0u64;
    while !input.0.is_empty() {
        match input.byte() % 8 {
            0..=2 => {
                // A write anywhere in the file: mostly the header and the
                // slot headers, where the numbers are.
                let off = u64::from(input.u32()) % full;
                let off = if input.byte() % 4 != 0 { off % 512 } else { off };
                let n = usize::from(input.byte() % 16) + 1;
                let bytes = input.bytes(n).to_vec();
                let n = bytes.len().min((full - off) as usize);
                file.write_all_at(&bytes[..n], off).expect("write");
            }
            3 => {
                // A slot's sequence word at the top of its range.
                let slot = u64::from(input.byte()) % u64::from(layout.slots);
                let seq = TOPS[usize::from(input.byte()) % TOPS.len()];
                file.write_all_at(&seq.to_ne_bytes(), HEADER + slot * stride(&layout) + SL_SEQ)
                    .expect("write");
            }
            4 | 5 => {
                let Some(prod) = p.as_mut() else { continue };
                let wanted = input.byte() % 4 != 0;
                if wanted {
                    want(&file, &c);
                }
                let side = 2 * layout.max_width.max(layout.max_height);
                let w = 1 + u32::from(input.byte()) % side;
                let h = 1 + u32::from(input.byte()) % side;
                let y = vec![input.byte(); (w * h) as usize];
                let uv = vec![128u8; nv12_len(w, h) - (w * h) as usize];
                let mut f = Nv12Frame::new(&y, w as usize, &uv, 2 * (w as usize).div_ceil(2), w, h);
                f.rotation = [0, 90, 180, 270][usize::from(input.byte() % 4)];
                f.color = Color::from_u32(u32::from(input.byte() % 3)).expect("a colour");
                // A second apart: no rate a layout allows drops it.
                ts += 1_000_000_000;
                f.timestamp_ns = ts;
                match prod.push(&f) {
                    Ok(Pushed::Written {
                        frame_no,
                        width,
                        height,
                    }) => check_written(&file, &mut c, prod, frame_no, width, height),
                    Ok(other) => assert!(!wanted, "{other:?} though wanted"),
                    Err(e) => panic!("an error from a whole file: {e}"),
                }
            }
            6 => {
                // The other process shrinks the file, then grows it back.
                let len = u64::from(input.u32()) % full;
                file.set_len(len).expect("shrink");
                if let Some(prod) = p.as_mut() {
                    let (y, uv) = (vec![1u8; 16 * 16], vec![2u8; 16 * 8]);
                    assert!(prod.push(&Nv12Frame::new(&y, 16, &uv, 16, 16, 16)).is_err());
                    assert!(!prod.wants_frames() && !prod.stop_requested());
                    prod.start();
                    let _ = prod.layout();
                }
                file.set_len(full).expect("grow");
            }
            _ => {
                // The producer goes and another attaches, to whatever the
                // file holds now.
                drop(p.take());
                p = Producer::attach(&path).ok();
                if let Some(p) = &p {
                    p.start();
                    let _ = (p.wants_frames(), p.stop_requested(), p.layout());
                }
            }
        }
    }
    drop(p);
    drop(c);
});
