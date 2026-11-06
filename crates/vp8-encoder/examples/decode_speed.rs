//! vp8-decoder speed on camera-like streams made by this encoder.
//!
//! For each size and bitrate it encodes synthetic camera content (a slow
//! pan, a moving object and sensor noise; see `tests/common/camera.rs`),
//! then decodes the stream over and over on one thread and reports frames
//! per second and milliseconds per frame. It also prints a checksum of
//! every decoded frame, so two builds of the decoder can be checked for
//! identical output.
//!
//!   cargo run --release -p vp8-encoder --example decode_speed -- \
//!       [--sizes 640x360,320x180,1280x720] [--rates 300000,600000,1200000] \
//!       [--frames 150] [--seconds 3] [--ivf-dir DIR]
//!
//! `--ivf-dir` also writes each stream as an IVF file, for
//! `cargo bench -p vp8-decoder --bench decode` or a profiler.

#[path = "../tests/common/camera.rs"]
mod camera;

use camera::Camera;
use std::time::{Duration, Instant};
use vp8_decoder::Decoder;
use vp8_encoder::{Encoder, EncoderConfig, Picture};

struct Args {
    sizes: Vec<(u32, u32)>,
    rates: Vec<u32>,
    frames: usize,
    seconds: f64,
    ivf_dir: Option<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        sizes: vec![(320, 180), (640, 360), (1280, 720)],
        rates: vec![300_000, 600_000, 1_200_000],
        frames: 150,
        seconds: 3.0,
        ivf_dir: None,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let value = args.get(i + 1).expect("every option takes a value");
        match args[i].as_str() {
            "--sizes" => {
                a.sizes = value
                    .split(',')
                    .map(|s| {
                        let (w, h) = s.split_once('x').expect("sizes look like 640x360");
                        (w.parse().expect("width"), h.parse().expect("height"))
                    })
                    .collect()
            }
            "--rates" => {
                a.rates = value
                    .split(',')
                    .map(|r| r.parse().expect("rate in bit/s"))
                    .collect()
            }
            "--frames" => a.frames = value.parse().expect("frame count"),
            "--seconds" => a.seconds = value.parse().expect("seconds"),
            "--ivf-dir" => a.ivf_dir = Some(value.clone()),
            other => panic!("unknown option {other}"),
        }
        i += 2;
    }
    a
}

/// FNV-1a, 64 bits: enough to tell two decoders' output apart.
fn fnv(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= b as u64;
        *hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
}

fn write_ivf(path: &str, w: u32, h: u32, frames: &[Vec<u8>]) {
    let mut out = Vec::new();
    out.extend_from_slice(b"DKIF");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"VP80");
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&30u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for (pts, f) in frames.iter().enumerate() {
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(&(pts as u64).to_le_bytes());
        out.extend_from_slice(f);
    }
    std::fs::write(path, out).unwrap_or_else(|e| panic!("{path}: {e}"));
}

fn main() {
    let args = parse_args();
    println!(
        "| size | target kbit/s | got kbit/s | frames | fps | ms/frame | keyframe ms | checksum |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for &(w, h) in &args.sizes {
        let cam = Camera::new(w as usize, h as usize, args.frames);
        for &rate in &args.rates {
            let mut enc = Encoder::new(EncoderConfig::new(w, h, 30, rate)).expect("encoder");
            let mut stream = Vec::with_capacity(args.frames);
            for t in 0..args.frames {
                let src = cam.frame(t);
                let pic = Picture::from_i420(&src, w, h).expect("picture");
                if let Some(f) = enc.encode(&pic, false).expect("encode") {
                    stream.push(f.data);
                }
            }
            if let Some(dir) = &args.ivf_dir {
                write_ivf(&format!("{dir}/camera-{w}x{h}-{rate}.ivf"), w, h, &stream);
            }
            let bytes: usize = stream.iter().map(|f| f.len()).sum();
            let kbps = bytes as f64 * 8.0 / (args.frames as f64 / 30.0) / 1000.0;

            // One checked pass: the checksum and the keyframe's own time.
            let mut dec = Decoder::new();
            let mut sum = 0xcbf2_9ce4_8422_2325u64;
            let t0 = Instant::now();
            let first = dec.decode(&stream[0]).expect("decode");
            let key_ms = t0.elapsed().as_secs_f64() * 1000.0;
            if let Some(p) = first {
                fnv(&mut sum, &p.to_i420());
            }
            for f in &stream[1..] {
                if let Some(p) = dec.decode(f).expect("decode") {
                    fnv(&mut sum, &p.to_i420());
                }
            }

            // Timed passes, each from the keyframe, for at least `seconds`.
            let mut decoded = 0u64;
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs_f64(args.seconds) {
                let mut dec = Decoder::new();
                for f in &stream {
                    let p = dec.decode(f).expect("decode");
                    std::hint::black_box(&p);
                    decoded += 1;
                }
            }
            let secs = start.elapsed().as_secs_f64();
            println!(
                "| {w}x{h} | {} | {kbps:.0} | {} | {:.1} | {:.3} | {key_ms:.2} | {sum:016x} |",
                rate / 1000,
                stream.len(),
                decoded as f64 / secs,
                secs * 1000.0 / decoded as f64,
            );
        }
    }
}
