//! A producer attaching to any file: the input is the file. An error or a
//! working producer, never a panic or a fault. When it attaches, it pushes
//! a frame too.
//!
//! Run from this folder: `cargo +nightly fuzz run attach`.
#![no_main]

use kessel_frame_ring::{Nv12Frame, Producer};
use libfuzzer_sys::fuzz_target;
use std::sync::atomic::{AtomicU32, Ordering};

fuzz_target!(|data: &[u8]| {
    static N: AtomicU32 = AtomicU32::new(0);
    let path = std::env::temp_dir().join(format!(
        "kfr-fuzz-attach-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, data).expect("write the file");
    if let Ok(mut p) = Producer::attach(&path) {
        p.start();
        let (y, uv) = (vec![7u8; 32 * 16], vec![128u8; 32 * 8]);
        let _ = p.push(&Nv12Frame::new(&y, 32, &uv, 32, 32, 16));
        let _ = (p.wants_frames(), p.stop_requested(), p.layout());
    }
    std::fs::remove_file(&path).expect("remove the file");
});
