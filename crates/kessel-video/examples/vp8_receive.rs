//! The VP8 receive path through kessel-video's public API, stage by stage,
//! on camera-like content (vp8-encoder's test scene) at 640x360 by default.
//!
//!   cargo run --release -p kessel-video --features system --example vp8_receive -- \
//!       [--size 640x360] [--rate 600000] [--frames 150] [--seconds 3]
//!
//! Stages: the raw vp8-decoder decode, its I420 copy, the backend's
//! decoder (thread, queues and hand-off) at full speed and paced at 30 fps,
//! `convert` to I420, NV12 and BGRA, the Apple backend's `convert` to
//! Native, and an I420 to NV12 interleave as an app might do it. On Apple
//! it also reports the QoS class of the decoder's thread (read from inside
//! the wake callback), whether it inherits one, and what each class costs.

#[path = "../../vp8-encoder/tests/common/camera.rs"]
mod camera;

use kessel_video::{
    convert, DecoderConfig, EncodedInput, PixelFormat, SystemConfig, VideoBackend, VideoCodec,
    VideoFrame, Wake,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Args {
    w: u32,
    h: u32,
    rate: u32,
    frames: usize,
    seconds: f64,
}

fn parse_args() -> Args {
    let mut a = Args {
        w: 640,
        h: 360,
        rate: 600_000,
        frames: 150,
        seconds: 3.0,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    for pair in args.chunks(2) {
        let value = pair.get(1).expect("every option takes a value");
        match pair[0].as_str() {
            "--size" => {
                let (w, h) = value.split_once('x').expect("size looks like 640x360");
                a.w = w.parse().expect("width");
                a.h = h.parse().expect("height");
            }
            "--rate" => a.rate = value.parse().expect("rate in bit/s"),
            "--frames" => a.frames = value.parse().expect("frame count"),
            "--seconds" => a.seconds = value.parse().expect("seconds"),
            other => panic!("unknown option {other}"),
        }
    }
    a
}

/// Runs `f` repeatedly for at least `seconds`; returns ms per call.
fn time_ms(seconds: f64, mut f: impl FnMut()) -> f64 {
    let mut n = 0u64;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs_f64(seconds) || n == 0 {
        f();
        n += 1;
    }
    start.elapsed().as_secs_f64() * 1000.0 / n as f64
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod qos {
    pub const USER_INTERACTIVE: u32 = 0x21;
    pub const USER_INITIATED: u32 = 0x19;
    pub const DEFAULT: u32 = 0x15;
    pub const UTILITY: u32 = 0x11;
    pub const BACKGROUND: u32 = 0x09;
    extern "C" {
        fn qos_class_self() -> u32;
        fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
    }
    pub fn current() -> u32 {
        // SAFETY: reads the calling thread's QoS class; no arguments.
        unsafe { qos_class_self() }
    }
    pub fn set(class: u32) {
        // SAFETY: sets the calling thread's own QoS class.
        let r = unsafe { pthread_set_qos_class_self_np(class, 0) };
        assert_eq!(r, 0, "pthread_set_qos_class_self_np");
    }
    pub fn name(class: u32) -> &'static str {
        match class {
            0x21 => "user-interactive",
            0x19 => "user-initiated",
            0x15 => "default",
            0x11 => "utility",
            0x09 => "background",
            0 => "unspecified",
            _ => "other",
        }
    }
}

/// The decoder thread's QoS, as the wake callback (run on that thread) saw it.
static DECODER_QOS: AtomicU32 = AtomicU32::new(u32::MAX);

/// The decoder's wake callback, which runs on the decoder's thread: it
/// records that thread's QoS class and, with `force`, first moves the
/// thread to that class (to measure what a decoder thread at each class
/// costs; kessel-video itself sets user-initiated since 0.1.4).
fn wake(force: Option<u32>) -> Wake {
    Arc::new(move || {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            if let Some(class) = force {
                if qos::current() != class {
                    qos::set(class);
                }
            }
            DECODER_QOS.store(qos::current(), Ordering::Relaxed);
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        let _ = force;
    })
}

/// Feeds the stream through a backend decoder. `pace`: submit one frame
/// per 1/30 s (as a network delivers them) instead of as fast as the
/// decoder takes them. Returns (frames out per second, mean ms from
/// submit to the frame being ready, frames dropped or refused).
fn through_decoder(
    backend: &Arc<dyn VideoBackend>,
    stream: &[(Vec<u8>, bool)],
    cfg: DecoderConfig,
    pace: bool,
    seconds: f64,
    force_qos: Option<u32>,
) -> (f64, f64, u64) {
    let mut dec = backend.decoder(cfg, wake(force_qos)).expect("decoder");
    let mut out = 0u64;
    let mut lost = 0u64;
    let mut latency = Duration::ZERO;
    let start = Instant::now();
    let mut i = 0usize;
    let mut next = Instant::now();
    while start.elapsed() < Duration::from_secs_f64(seconds) || !i.is_multiple_of(stream.len()) {
        let (data, key) = &stream[i % stream.len()];
        if i.is_multiple_of(stream.len()) && i > 0 {
            dec.reset();
        }
        if pace {
            while Instant::now() < next {
                std::thread::sleep(Duration::from_micros(200));
            }
            next += Duration::from_micros(33_333);
        }
        let t = Instant::now();
        let input = EncodedInput::new(VideoCodec::Vp8, data.clone(), *key, 0);
        loop {
            match dec.submit(input.clone()) {
                Ok(()) => break,
                Err(kessel_video::DecodeError::Busy) if !pace => std::thread::yield_now(),
                Err(_) => {
                    lost += 1;
                    break;
                }
            }
        }
        // Wait for this frame's output (one in flight keeps latency honest).
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match dec.poll() {
                Some(Ok(f)) => {
                    std::hint::black_box(&f);
                    out += 1;
                    latency += t.elapsed();
                    break;
                }
                Some(Err(e)) => panic!("decode: {e}"),
                None if Instant::now() > deadline => panic!("no output in 2 s"),
                None => std::thread::yield_now(),
            }
        }
        i += 1;
    }
    let secs = start.elapsed().as_secs_f64();
    (
        out as f64 / secs,
        latency.as_secs_f64() * 1000.0 / out.max(1) as f64,
        lost,
    )
}

fn main() {
    let a = parse_args();
    let (w, h) = (a.w, a.h);
    let cam = camera::Camera::new(w as usize, h as usize, a.frames);
    let mut enc = vp8_encoder::Encoder::new(vp8_encoder::EncoderConfig::new(w, h, 30, a.rate))
        .expect("encoder");
    let mut stream = Vec::new();
    for t in 0..a.frames {
        let src = cam.frame(t);
        let pic = vp8_encoder::Picture::from_i420(&src, w, h).expect("picture");
        if let Some(f) = enc.encode(&pic, false).expect("encode") {
            stream.push((f.data, f.keyframe));
        }
    }
    let kbps = stream.iter().map(|f| f.0.len()).sum::<usize>() as f64 * 8.0 * 30.0
        / stream.len() as f64
        / 1000.0;
    println!(
        "{w}x{h}, {} frames at {kbps:.0} kbit/s (camera-like content)",
        stream.len()
    );
    println!("| stage | ms/frame | fps |");
    println!("|---|---|---|");
    let row = |name: &str, ms: f64| println!("| {name} | {ms:.3} | {:.0} |", 1000.0 / ms);

    // 1. vp8-decoder alone, then with the I420 copy kessel-video makes.
    let mut dec = vp8_decoder::Decoder::new();
    let mut k = 0;
    let ms = time_ms(a.seconds, || {
        if k % stream.len() == 0 {
            dec = vp8_decoder::Decoder::new();
        }
        std::hint::black_box(dec.decode(&stream[k % stream.len()].0).expect("decode"));
        k += 1;
    });
    row("vp8-decoder decode", ms);
    let mut dec = vp8_decoder::Decoder::new();
    let mut k = 0;
    let ms = time_ms(a.seconds, || {
        if k % stream.len() == 0 {
            dec = vp8_decoder::Decoder::new();
        }
        if let Some(p) = dec.decode(&stream[k % stream.len()].0).expect("decode") {
            std::hint::black_box(p.to_i420());
        }
        k += 1;
    });
    row("vp8-decoder decode + to_i420 (Vp8Core)", ms);

    // 2. The backend's decoder: its thread, queues and hand-off.
    let backend = kessel_video::system(SystemConfig::default()).expect("backend");
    let cfg = DecoderConfig::new(VideoCodec::Vp8);
    let (fps, lat, lost) = through_decoder(&backend, &stream, cfg, false, a.seconds, None);
    println!(
        "| backend decoder, I420 out, back to back | {:.3} (submit to ready) | {fps:.0} ({lost} lost) |",
        lat
    );
    let (fps, lat, lost) = through_decoder(&backend, &stream, cfg, true, a.seconds, None);
    println!(
        "| backend decoder, I420 out, paced at 30 fps | {:.3} (submit to ready) | {fps:.0} ({lost} lost) |",
        lat
    );
    let mut fit = cfg;
    fit.fit = Some((w / 2, h / 2));
    let (fps, lat, lost) = through_decoder(&backend, &stream, fit, false, a.seconds, None);
    println!(
        "| backend decoder, I420 out, fit {}x{} | {:.3} (submit to ready) | {fps:.0} ({lost} lost) |",
        w / 2,
        h / 2,
        lat
    );
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let q = DECODER_QOS.load(Ordering::Relaxed);
        println!(
            "| decoder thread QoS (created from the main thread, {}) | {} | |",
            qos::name(qos::current()),
            qos::name(q)
        );
    }

    // 3. Conversions of one decoded frame.
    let mut dec = backend.decoder(cfg, wake(None)).expect("decoder");
    dec.submit(EncodedInput::new(
        VideoCodec::Vp8,
        stream[0].0.clone(),
        true,
        0,
    ))
    .expect("submit");
    let frame: VideoFrame = loop {
        if let Some(r) = dec.poll() {
            break r.expect("decode");
        }
        std::thread::yield_now();
    };
    for (name, fmt) in [
        ("convert to I420 (same size)", PixelFormat::I420),
        ("convert to NV12", PixelFormat::Nv12),
        ("convert to BGRA", PixelFormat::Bgra),
    ] {
        row(
            name,
            time_ms(a.seconds, || {
                std::hint::black_box(convert(&frame, w, h, fmt).expect("convert"));
            }),
        );
    }
    row(
        &format!("convert to I420 at {}x{}", w / 2, h / 2),
        time_ms(a.seconds, || {
            std::hint::black_box(convert(&frame, w / 2, h / 2, PixelFormat::I420).expect("convert"));
        }),
    );
    match backend.convert(&frame, w, h, PixelFormat::Native) {
        Ok(_) => row(
            "backend convert to Native",
            time_ms(a.seconds, || {
                std::hint::black_box(
                    backend
                        .convert(&frame, w, h, PixelFormat::Native)
                        .expect("native"),
                );
            }),
        ),
        Err(e) => println!("| backend convert to Native | unsupported here: {e} | |"),
    }

    // 4. An app's own I420 to NV12 interleave (a plain loop).
    let planes = frame.planes().expect("CPU planes");
    let (cw, ch) = (w.div_ceil(2) as usize, h.div_ceil(2) as usize);
    let mut nv12 = vec![0u8; (w * h) as usize + 2 * cw * ch];
    row(
        "app I420 to NV12 interleave",
        time_ms(a.seconds, || {
            for y in 0..h as usize {
                let r = planes.row(0, y, w, h);
                nv12[y * w as usize..(y + 1) * w as usize].copy_from_slice(&r[..w as usize]);
            }
            let base = (w * h) as usize;
            for y in 0..ch {
                let (u, v) = (planes.row(1, y, w, h), planes.row(2, y, w, h));
                let dst = &mut nv12[base + y * 2 * cw..base + (y + 1) * 2 * cw];
                for x in 0..cw {
                    dst[2 * x] = u[x];
                    dst[2 * x + 1] = v[x];
                }
            }
            std::hint::black_box(&nv12);
        }),
    );

    // 5. Apple: kessel-video's decoder thread moves itself to user-initiated
    // (0.1.4). Is that what it runs at, whatever thread creates the decoder?
    // And what does each class cost, back to back and paced like a network
    // stream?
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        for class in [qos::BACKGROUND, qos::USER_INTERACTIVE] {
            let backend = backend.clone();
            let stream = stream.clone();
            let seconds = a.seconds.min(1.0);
            let q = std::thread::spawn(move || {
                qos::set(class);
                through_decoder(&backend, &stream, cfg, false, seconds, None);
                DECODER_QOS.load(Ordering::Relaxed)
            })
            .join()
            .expect("thread");
            println!(
                "| decoder created on a {} thread: its thread's QoS | {} | |",
                qos::name(class),
                qos::name(q)
            );
        }
        for class in [
            qos::BACKGROUND,
            qos::UTILITY,
            qos::DEFAULT,
            qos::USER_INITIATED,
            qos::USER_INTERACTIVE,
        ] {
            for pace in [false, true] {
                let (fps, lat, lost) =
                    through_decoder(&backend, &stream, cfg, pace, a.seconds, Some(class));
                println!(
                    "| decoder thread at {}, {} | {lat:.3} (submit to ready) | {fps:.0} ({lost} lost) |",
                    qos::name(class),
                    if pace { "paced at 30 fps" } else { "back to back" }
                );
            }
        }
    }
}
