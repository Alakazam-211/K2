//! A consumer's ring that the other process scribbles on: the input is a
//! layout pick, then a list of writes into the file (through a second open
//! of it, as the producer's process would) and reads in between. Any panic
//! or fault is a finding, and so is a frame the consumer hands out that
//! doesn't fit its layout.
//!
//! Run from this folder: `cargo +nightly fuzz run consumer`.
#![no_main]

use kessel_frame_ring::{nv12_len, Consumer, FrameInfo, Layout};
use libfuzzer_sys::fuzz_target;
use std::fs::OpenOptions;
use std::os::unix::fs::FileExt;
use std::sync::atomic::{AtomicU32, Ordering};

fn check(layout: &Layout, info: &FrameInfo, buf: &[u8]) {
    let (w, h) = (info.width, info.height);
    let fits = (w <= layout.max_width && h <= layout.max_height)
        || (w <= layout.max_height && h <= layout.max_width);
    assert!(w >= 1 && h >= 1 && fits, "{info:?} outside {layout:?}");
    assert!(matches!(info.rotation, 0 | 90 | 180 | 270), "{info:?}");
    assert_eq!(buf.len(), nv12_len(w, h), "{info:?}");
    assert_eq!(buf.len(), info.len());
    assert!(info.uv_offset() <= buf.len());
}

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
        "kfr-fuzz-{}-{}",
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
    let mut buf = Vec::new();
    let mut last = None;
    while !input.0.is_empty() {
        match input.byte() % 8 {
            0..=2 => {
                // A write anywhere in the file: mostly the header and the
                // slot headers, where the numbers are.
                let off = u64::from(input.u32()) % full;
                let off = if input.byte() % 4 != 0 { off % 512 } else { off };
                let n = usize::from(input.byte() % 16) + 1;
                let bytes = input.bytes(n).to_vec();
                file.write_at(&bytes, off).expect("write");
            }
            3 => {
                if let Some(info) = c.try_read(&mut buf) {
                    check(&layout, &info, &buf);
                    // try_read hands out only a frame other than the last
                    // one handed out.
                    assert_ne!(last, Some(info.frame_no), "handed out twice");
                    last = Some(info.frame_no);
                }
            }
            4 => {
                if let Some(info) = c.read_latest(&mut buf) {
                    check(&layout, &info, &buf);
                    last = Some(info.frame_no);
                }
            }
            5 => {
                let _ = c.producer();
                let _ = c.stats();
                assert_eq!(c.layout(), layout);
            }
            6 => {
                // The other process shrinks the file, then grows it back.
                let len = u64::from(input.u32()) % full;
                file.set_len(len).expect("shrink");
                assert!(c.try_read(&mut buf).is_none());
                assert!(c.read_latest(&mut buf).is_none());
                assert!(c.producer().is_err());
                c.touch();
                file.set_len(full).expect("grow");
            }
            _ => {
                c.set_wanted(input.byte() % 2 == 0);
                c.request_stop();
            }
        }
    }
    drop(c);
});
