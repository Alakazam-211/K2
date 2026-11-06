//! Decode speed: decodes each IVF file in `VP8_BENCH_IVF` (a comma-separated
//! list) repeatedly on one thread for at least `VP8_BENCH_SECONDS` (default 5)
//! and prints frames per second.
//!
//!   VP8_BENCH_IVF=a.ivf,b.ivf cargo bench -p vp8-decoder --bench decode

use std::time::{Duration, Instant};
use vp8_decoder::ivf::IvfReader;
use vp8_decoder::Decoder;

fn main() {
    let list = std::env::var("VP8_BENCH_IVF").expect("set VP8_BENCH_IVF to one or more IVF files");
    let seconds: u64 = std::env::var("VP8_BENCH_SECONDS")
        .map(|s| s.parse().expect("VP8_BENCH_SECONDS is a whole number"))
        .unwrap_or(5);
    for path in list.split(',').filter(|p| !p.is_empty()) {
        let data = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let reader = IvfReader::new(&data).unwrap_or_else(|e| panic!("{path}: {e}"));
        let header = reader.header();
        let frames: Vec<&[u8]> = reader.map(|f| f.expect("frame").data).collect();
        let bytes: usize = frames.iter().map(|f| f.len()).sum();
        let mut decoded = 0u64;
        let mut keyframes = 0u64;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(seconds) {
            let mut dec = Decoder::new();
            for f in &frames {
                if f[0] & 1 == 0 {
                    keyframes += 1;
                }
                let pic = dec.decode(f).unwrap_or_else(|e| panic!("{path}: {e}"));
                std::hint::black_box(&pic);
                decoded += 1;
            }
        }
        let secs = start.elapsed().as_secs_f64();
        let kbps = bytes as f64 * 8.0 / frames.len() as f64 * 30.0 / 1000.0;
        println!(
            "[bench] {path}: {}x{}, {} frames ({:.0} kbit/s at 30 fps, {:.1}% keyframes): {:.1} fps, {:.2} ms/frame",
            header.width,
            header.height,
            frames.len(),
            kbps,
            keyframes as f64 * 100.0 / decoded as f64,
            decoded as f64 / secs,
            secs * 1000.0 / decoded as f64
        );
    }
}
