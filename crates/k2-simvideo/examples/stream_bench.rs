//! Before / after for the Mobile tab's device stream.
//!
//!     cargo run --release -p k2-simvideo --example stream_bench
//!
//! A synthetic app screen (411x923 points, a list scrolling 6 pt per frame,
//! a new frame every 16.7 ms: the worst case, continuous motion) goes
//! through the daemon's real encoders (the `image` crate's JPEG at q70, as
//! `k2_core::sim::encode`; VideoToolbox H.264 on a Mac) and the real
//! [`AckWindow`], over a simulated link (RTT, bandwidth, FIFO). The viewer's
//! decode + draw time is an assumption (see `JPEG_DRAW_MS`, `H264_DRAW_MS`),
//! everything else is measured or the daemon's own logic.
//!
//! Reported per scenario: frames drawn per second, capture → drawn latency
//! (mean, p95), and link bandwidth.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use k2_simvideo::flow::{AckWindow, RateCtl};
use k2_simvideo::{bitrate_for, EncoderConfig, VideoEncoder};

const W: u32 = 411;
const H: u32 = 923;
const SRC_MS: f64 = 1000.0 / 60.0;
const SIM_MS: f64 = 10_000.0;
const STEP_MS: f64 = 0.25;
/// Viewer-side decode + draw (assumed): createImageBitmap + drawImage of a
/// ~400x900 JPEG in a webview, and a hardware WebCodecs decode + drawImage.
const JPEG_DRAW_MS: f64 = 5.0;
const H264_DRAW_MS: f64 = 2.0;
const JPEG_QUALITY: u8 = 70;

/// The second table's scene: every row's text and avatar change on every
/// frame (a worst case for H.264, like a video playing full screen).
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The scene at source frame `k`: a white list with a coloured header,
/// avatars, two lines of "text" per row, scrolling 6 px per frame.
fn scene(k: u64) -> Vec<u8> {
    let off = (k * 6) as u32;
    let churn = if BUSY.load(std::sync::atomic::Ordering::SeqCst) { (k as u32).wrapping_mul(0x9E37_79B9) } else { 0 };
    let mut v = vec![255u8; (W * H * 3) as usize];
    let put = |v: &mut Vec<u8>, x: u32, y: u32, c: [u8; 3]| {
        let i = ((y * W + x) * 3) as usize;
        v[i..i + 3].copy_from_slice(&c);
    };
    for y in 0..H {
        if y < 90 {
            for x in 0..W {
                put(&mut v, x, y, [33, 99, 214]);
            }
            continue;
        }
        let cy = y - 90 + off;
        let row = cy / 64;
        let ry = cy % 64;
        let seed = row.wrapping_mul(2_654_435_761) ^ churn;
        for x in 0..W {
            let c = if ry == 63 {
                [225, 225, 225]
            } else if (12..52).contains(&ry) && (16..56).contains(&x) {
                let h = (seed >> 8) as u8;
                [h, 255 - h / 2, 120 + h / 3]
            } else if (14..26).contains(&ry) || (34..44).contains(&ry) {
                // "Glyphs": 7 px cells, dark when the cell's hash says so,
                // line lengths vary per row.
                let line = if ry < 30 { 0 } else { 1 };
                let len = 140 + (seed >> (line * 5)) % 200;
                let cell = x / 7;
                let ink = (cell.wrapping_mul(97) ^ seed.rotate_left(line * 3) ^ (ry / 3)) % 5 != 0 && x % 7 < 5;
                if (72..72 + len).contains(&x) && ink {
                    if line == 0 { [20, 20, 20] } else { [110, 110, 110] }
                } else {
                    [255, 255, 255]
                }
            } else {
                [255, 255, 255]
            };
            put(&mut v, x, y, c);
        }
    }
    v
}

fn jpeg(rgb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 * 1024);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode(rgb, W, H, image::ExtendedColorType::Rgb8)
        .expect("jpeg");
    out
}

#[derive(Clone, Copy)]
struct Scenario {
    name: &'static str,
    rtt_ms: f64,
    mbps: f64,
    remote: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Codec {
    Jpeg,
    H264,
}

#[derive(Clone, Copy)]
struct Mode {
    label: &'static str,
    codec: Codec,
    window: u32,
    fps: u32,
}

struct Row {
    fps: f64,
    lat_mean: f64,
    lat_p95: f64,
    mbps: f64,
    kb_per_frame: f64,
    keys: u32,
}

/// JPEG size and encode time per source frame (scenes are rebuilt on
/// demand; 600 of them would be ~700 MB).
struct Caches {
    jpegs: HashMap<u64, (usize, f64)>,
}

impl Caches {
    fn jpeg(&mut self, k: u64) -> (usize, f64) {
        *self.jpegs.entry(k).or_insert_with(|| {
            let s = scene(k);
            let t = Instant::now();
            let j = jpeg(&s);
            (j.len(), t.elapsed().as_secs_f64() * 1000.0)
        })
    }
}

struct InFlight {
    seq: u32,
    capture: f64,
    arrive: f64,
}

fn simulate(sc: Scenario, m: Mode, caches: &mut Caches) -> Option<Row> {
    let base = Instant::now();
    let at = |t: f64| base + Duration::from_secs_f64(t / 1000.0);
    let gap = 1000.0 / m.fps as f64;
    let mut enc: Option<Box<dyn VideoEncoder>> = None;
    let mut rate = RateCtl::new(bitrate_for(W, H, m.fps, sc.remote));
    if m.codec == Codec::H264 {
        let cfg = EncoderConfig { width: W, height: H, fps: m.fps, bitrate_bps: rate.current() };
        enc = Some(k2_simvideo::open_h264(&cfg).ok()?);
    }
    let mut win = AckWindow::new(m.window);
    // The hub: publishes the newest source frame at the fps cap (JPEG
    // encoded on the hub, H.264 raw).
    let mut hub_next = 0.0f64;
    let mut published: Option<(u64, f64)> = None; // (k, ready at)
    let mut hub_busy_until = 0.0f64;
    let mut hub_pending: Option<(u64, f64)> = None;
    let mut last_sent_k: Option<u64> = None;
    let mut last_sent_t = -1e9f64;
    let mut enc_busy_until = 0.0f64;
    let mut link_free = 0.0f64;
    let mut client_free = 0.0f64;
    let mut arrivals: VecDeque<InFlight> = VecDeque::new();
    let mut acks: VecDeque<(f64, u32)> = VecDeque::new();
    let mut latencies: Vec<f64> = Vec::new();
    let mut bytes: u64 = 0;
    let mut sent: u64 = 0;
    let mut keys = 0u32;
    let mut seq: u32 = 0;
    let mut force_key = true;
    let mut t = 0.0f64;
    while t < SIM_MS {
        // Hub.
        if let Some(p) = hub_pending {
            if t >= hub_busy_until {
                published = Some(p);
                hub_pending = None;
            }
        }
        if hub_pending.is_none() && t >= hub_next {
            let k = (t / SRC_MS).floor() as u64;
            if published.map_or(true, |(pk, _)| pk != k) {
                let enc_ms = if m.codec == Codec::Jpeg { caches.jpeg(k).1 } else { 0.0 };
                hub_pending = Some((k, k as f64 * SRC_MS));
                hub_busy_until = t + enc_ms;
                hub_next = t + gap.max(enc_ms);
            }
        }
        // Acks back at the daemon.
        while acks.front().is_some_and(|(ta, _)| *ta <= t) {
            let (ta, s) = acks.pop_front().expect("ack");
            win.ack(s, at(ta));
            if m.codec == Codec::H264 {
                if let Some(b) = rate.update(win.congested(), at(ta)) {
                    enc.as_mut().expect("enc").set_rate(m.fps, b);
                }
            }
        }
        // The viewer loop: newest frame, window, fps cap, encoder free.
        if let Some((k, capture)) = published {
            let fresh = last_sent_k != Some(k);
            if fresh && win.can_send(m.fps) && t - last_sent_t >= gap && t >= enc_busy_until {
                seq += 1;
                let (size, send_at) = match m.codec {
                    Codec::Jpeg => (caches.jpeg(k).0, t),
                    Codec::H264 => {
                        let s = scene(k);
                        let e = enc.as_mut().expect("enc");
                        let t0 = Instant::now();
                        let c = e.encode_rgb(&s, force_key, capture as u32).expect("encode");
                        let ms = t0.elapsed().as_secs_f64() * 1000.0;
                        force_key = false;
                        enc_busy_until = t + ms;
                        match c {
                            Some(c) => {
                                if c.key {
                                    keys += 1;
                                }
                                (c.data.len(), t + ms)
                            }
                            None => {
                                last_sent_k = Some(k);
                                t += STEP_MS;
                                continue;
                            }
                        }
                    }
                };
                win.sent(seq, at(t));
                last_sent_k = Some(k);
                last_sent_t = t;
                let start = send_at.max(link_free);
                link_free = start + (size as f64 * 8.0) / (sc.mbps * 1000.0);
                arrivals.push_back(InFlight { seq, capture, arrive: link_free + sc.rtt_ms / 2.0 });
                bytes += size as u64;
                sent += 1;
            }
        }
        // The viewer: draw in order; a JPEG with a newer one already here
        // is skipped (acked, not drawn). H.264 decodes every frame.
        while arrivals.front().is_some_and(|f| f.arrive <= t && client_free <= t) {
            let f = arrivals.pop_front().expect("arrival");
            let newer_here = arrivals.front().is_some_and(|n| n.arrive <= t);
            let done = if m.codec == Codec::Jpeg && newer_here {
                t
            } else {
                let d = t.max(f.arrive) + if m.codec == Codec::Jpeg { JPEG_DRAW_MS } else { H264_DRAW_MS };
                if t > 1000.0 {
                    latencies.push(d - f.capture);
                }
                d
            };
            client_free = done;
            acks.push_back((done + sc.rtt_ms / 2.0, f.seq));
        }
        t += STEP_MS;
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).expect("ms"));
    let n = latencies.len().max(1) as f64;
    let secs = (SIM_MS - 1000.0) / 1000.0;
    Some(Row {
        fps: latencies.len() as f64 / secs,
        lat_mean: latencies.iter().sum::<f64>() / n,
        lat_p95: latencies.get(((latencies.len() as f64) * 0.95) as usize).copied().unwrap_or(0.0),
        mbps: bytes as f64 * 8.0 / (SIM_MS / 1000.0) / 1e6,
        kb_per_frame: bytes as f64 / sent.max(1) as f64 / 1024.0,
        keys,
    })
}

fn main() {
    run(false);
    run(true);
}

fn run(busy: bool) {
    BUSY.store(busy, std::sync::atomic::Ordering::SeqCst);
    let mut caches = Caches { jpegs: HashMap::new() };
    // Encoder costs on this machine.
    let mut jms = Vec::new();
    let mut jkb = Vec::new();
    for k in 0..60 {
        let (n, ms) = caches.jpeg(k);
        jms.push(ms);
        jkb.push(n as f64 / 1024.0);
    }
    let what = if busy { "every row's text and colour change every frame (worst case)" } else { "scrolling list" };
    println!("\n# Mobile stream bench ({}x{} pt, {what}, source 60 Hz)\n", W, H);
    println!("JPEG q{JPEG_QUALITY}: {:.1} KB/frame, {:.1} ms encode (mean of 60)", avg(&jkb), avg(&jms));
    match k2_simvideo::open_h264(&EncoderConfig { width: W, height: H, fps: 30, bitrate_bps: bitrate_for(W, H, 30, false) }) {
        Ok(mut e) => {
            let mut sizes = Vec::new();
            let mut ms = Vec::new();
            let mut key = 0usize;
            for k in 0..60u64 {
                let s = scene(k * 2);
                let t = Instant::now();
                let c = e.encode_rgb(&s, k == 0, (k * 33) as u32).expect("encode");
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
                if let Some(c) = c {
                    if k == 0 {
                        key = c.data.len();
                    } else {
                        sizes.push(c.data.len() as f64 / 1024.0);
                    }
                }
            }
            println!(
                "H.264 ({}) @30 fps, {:.2} Mbit/s target: keyframe {:.1} KB, P frames {:.1} KB, {:.1} ms encode (mean of 60)",
                e.name(),
                bitrate_for(W, H, 30, false) as f64 / 1e6,
                key as f64 / 1024.0,
                avg(&sizes),
                avg(&ms)
            );
        }
        Err(why) => println!("H.264: not on this host ({why})"),
    }
    println!("Viewer decode + draw (assumed): JPEG {JPEG_DRAW_MS} ms, H.264 {H264_DRAW_MS} ms\n");
    let scenarios = [
        Scenario { name: "local (loopback)", rtt_ms: 1.0, mbps: 1000.0, remote: false },
        Scenario { name: "LAN", rtt_ms: 5.0, mbps: 200.0, remote: true },
        Scenario { name: "Connect, 60 ms / 30 Mbit/s", rtt_ms: 60.0, mbps: 30.0, remote: true },
        Scenario { name: "Connect, 150 ms / 10 Mbit/s", rtt_ms: 150.0, mbps: 10.0, remote: true },
        Scenario { name: "Connect, 100 ms / 3 Mbit/s", rtt_ms: 100.0, mbps: 3.0, remote: true },
    ];
    println!("| link | mode | fps cap | window | fps drawn | latency mean | latency p95 | bandwidth | KB/frame | keyframes |");
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for sc in scenarios {
        let modes: Vec<Mode> = if sc.remote {
            vec![
                Mode { label: "today: JPEG, 1 in flight", codec: Codec::Jpeg, window: 1, fps: 8 },
                Mode { label: "today: JPEG, 1 in flight (max cap)", codec: Codec::Jpeg, window: 1, fps: 15 },
                Mode { label: "(a) JPEG, ack window", codec: Codec::Jpeg, window: 3, fps: 15 },
                Mode { label: "(b) H.264, ack window", codec: Codec::H264, window: 3, fps: 24 },
                Mode { label: "(b) H.264, ack window", codec: Codec::H264, window: 3, fps: 30 },
            ]
        } else {
            vec![
                Mode { label: "today: JPEG, 1 in flight", codec: Codec::Jpeg, window: 1, fps: 30 },
                Mode { label: "(a) JPEG, ack window", codec: Codec::Jpeg, window: 3, fps: 30 },
                Mode { label: "(b) H.264, ack window", codec: Codec::H264, window: 3, fps: 30 },
            ]
        };
        for m in modes {
            match simulate(sc, m, &mut caches) {
                Some(r) => println!(
                    "| {} | {} | {} | {} | {:.1} | {:.0} ms | {:.0} ms | {:.2} Mbit/s | {:.1} | {} |",
                    sc.name, m.label, m.fps, m.window, r.fps, r.lat_mean, r.lat_p95, r.mbps, r.kb_per_frame, r.keys
                ),
                None => println!("| {} | {} | {} | {} | n/a (no H.264 here) | | | | | |", sc.name, m.label, m.fps, m.window),
            }
        }
    }
}

fn avg(v: &[f64]) -> f64 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    }
}
