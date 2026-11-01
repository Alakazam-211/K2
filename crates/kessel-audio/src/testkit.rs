//! Test material and metrics, ported from the baseline measurement of
//! sonora against the C++ WebRTC library: synthetic rooms (image method), a laptop speaker
//! and chassis model, a fixed system delay, levels, and the ERLE /
//! convergence / level metrics. Speech is real (12 LibriSpeech utterances
//! shipped as Opus in tests/data) or synthetic (a formant synthesiser).

#![allow(dead_code)]

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

pub(crate) const FS: usize = 48_000;
/// Metric frame: 10 ms.
pub(crate) const FRAME: usize = FS / 100;

pub(crate) fn secs(s: f64) -> usize {
    (s * FS as f64) as usize
}

/// Deterministic xorshift64* RNG.
#[derive(Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub(crate) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub(crate) fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    pub(crate) fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.uniform()
    }
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    pub(crate) fn gauss(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// RBJ biquad.
pub(crate) struct Biquad {
    b: [f64; 3],
    a: [f64; 3],
    z: [f64; 2],
}

impl Biquad {
    pub(crate) fn highpass(fc: f64, q: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * fc / FS as f64;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        Biquad {
            b: [(1.0 + c) / 2.0 / a0, -(1.0 + c) / a0, (1.0 + c) / 2.0 / a0],
            a: [1.0, -2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }
    pub(crate) fn lowpass(fc: f64, q: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * fc / FS as f64;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        Biquad {
            b: [(1.0 - c) / 2.0 / a0, (1.0 - c) / a0, (1.0 - c) / 2.0 / a0],
            a: [1.0, -2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }
    pub(crate) fn bandpass(fc: f64, q: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * fc / FS as f64;
        let alpha = w0.sin() / (2.0 * q);
        let c = w0.cos();
        let a0 = 1.0 + alpha;
        Biquad {
            b: [alpha / a0, 0.0, -alpha / a0],
            a: [1.0, -2.0 * c / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }
    pub(crate) fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[1] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[2] * y;
        y
    }
    pub(crate) fn run(mut self, x: &[f32]) -> Vec<f32> {
        x.iter().map(|&v| self.process(v as f64) as f32).collect()
    }
}

pub(crate) fn energy(x: &[f32]) -> f64 {
    x.iter().map(|&v| (v as f64) * (v as f64)).sum()
}

pub(crate) fn db(p: f64) -> f64 {
    10.0 * p.max(1e-20).log10()
}

pub(crate) fn scale(x: &mut [f32], g: f64) {
    for v in x.iter_mut() {
        *v = (*v as f64 * g) as f32;
    }
}

pub(crate) fn add_into(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        *d += *s;
    }
}

pub(crate) fn delay(x: &[f32], d: usize) -> Vec<f32> {
    let mut y = vec![0.0f32; x.len()];
    if d < x.len() {
        y[d..].copy_from_slice(&x[..x.len() - d]);
    }
    y
}

/// Linear convolution of `x` with `h`, truncated to `x.len()`.
pub(crate) fn fft_convolve(x: &[f32], h: &[f32]) -> Vec<f32> {
    let n = (x.len() + h.len()).next_power_of_two();
    let mut planner = FftPlanner::<f64>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut a: Vec<Complex<f64>> = (0..n)
        .map(|i| Complex::new(x.get(i).copied().unwrap_or(0.0) as f64, 0.0))
        .collect();
    let mut b: Vec<Complex<f64>> = (0..n)
        .map(|i| Complex::new(h.get(i).copied().unwrap_or(0.0) as f64, 0.0))
        .collect();
    fwd.process(&mut a);
    fwd.process(&mut b);
    for (p, q) in a.iter_mut().zip(b.iter()) {
        *p *= *q;
    }
    inv.process(&mut a);
    let s = 1.0 / n as f64;
    a[..x.len()].iter().map(|c| (c.re * s) as f32).collect()
}

pub(crate) fn white(rng: &mut Rng, n: usize) -> Vec<f32> {
    (0..n).map(|_| rng.gauss() as f32).collect()
}

/// Pink noise (Paul Kellet's refined filter).
pub(crate) fn pink(rng: &mut Rng, n: usize) -> Vec<f32> {
    let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    (0..n)
        .map(|_| {
            let w = rng.gauss();
            b0 = 0.99886 * b0 + w * 0.0555179;
            b1 = 0.99332 * b1 + w * 0.0750759;
            b2 = 0.96900 * b2 + w * 0.1538520;
            b3 = 0.86650 * b3 + w * 0.3104856;
            b4 = 0.55000 * b4 + w * 0.5329522;
            b5 = -0.7616 * b5 - w * 0.0168980;
            let p = b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362;
            b6 = w * 0.115926;
            p as f32
        })
        .collect()
}

/// Active speech level (dBFS): frames above p95 - 25 dB.
pub(crate) fn active_level_db(x: &[f32]) -> f64 {
    let mut p: Vec<f64> = x.chunks(FRAME).map(|c| energy(c) / c.len() as f64).collect();
    if p.is_empty() {
        return -200.0;
    }
    let mut s = p.clone();
    s.sort_by(|a, b| a.total_cmp(b));
    let p95 = s[(s.len() * 95 / 100).min(s.len() - 1)];
    let th = (p95 * 10f64.powf(-2.5)).max(1e-9);
    p.retain(|&v| v > th);
    if p.is_empty() {
        return -200.0;
    }
    db(p.iter().sum::<f64>() / p.len() as f64)
}

pub(crate) fn set_active_level(x: &mut [f32], target_db: f64) {
    let l = active_level_db(x);
    scale(x, 10f64.powf((target_db - l) / 20.0));
}

// ---------------------------------------------------------------------------
// Real speech: LibriSpeech test-clean (CC BY 4.0), see tests/data/README.md
// ---------------------------------------------------------------------------

pub(crate) const FAR_POOL: u8 = 0;
pub(crate) const NEAR_POOL: u8 = 1;

pub(crate) struct Utterance {
    pub(crate) pool: u8,
    pub(crate) speaker: u32,
    pub(crate) pcm: Vec<f32>,
}

static CORPUS: std::sync::OnceLock<Vec<Utterance>> = std::sync::OnceLock::new();

fn rd_u16(b: &[u8], p: &mut usize) -> u16 {
    let v = u16::from_le_bytes([b[*p], b[*p + 1]]);
    *p += 2;
    v
}

fn rd_u32(b: &[u8], p: &mut usize) -> u32 {
    let v = u32::from_le_bytes([b[*p], b[*p + 1], b[*p + 2], b[*p + 3]]);
    *p += 4;
    v
}

/// The test corpus, decoded once (48 kHz, active level about -26 dBFS).
pub(crate) fn corpus() -> &'static [Utterance] {
    CORPUS.get_or_init(|| {
        let b: &[u8] = include_bytes!("../tests/data/librispeech-test-clean.kaop");
        assert_eq!(&b[..4], b"KAOP", "corpus magic");
        assert_eq!(b[4], 1, "corpus version");
        let mut p = 5;
        let n = rd_u16(b, &mut p);
        let mut out = Vec::new();
        for _ in 0..n {
            let pool = b[p];
            p += 1;
            let speaker = rd_u32(b, &mut p);
            let len = rd_u32(b, &mut p) as usize;
            let packets = rd_u32(b, &mut p);
            let mut dec = crate::codec::OpusDec::new().expect("decoder");
            let mut pcm = vec![0.0f32; packets as usize * crate::frame::FRAME_SAMPLES];
            for k in 0..packets as usize {
                let l = rd_u16(b, &mut p) as usize;
                let r = k * crate::frame::FRAME_SAMPLES..(k + 1) * crate::frame::FRAME_SAMPLES;
                let how = dec.decode(&b[p..p + l], &mut pcm[r]);
                assert_eq!(how, crate::codec::Decoded::Packet, "corpus packet {k} of speaker {speaker}");
                p += l;
            }
            pcm.truncate(len);
            out.push(Utterance { pool, speaker, pcm });
        }
        assert_eq!(p, b.len(), "trailing corpus bytes");
        out
    })
}

/// A talker track from one pool: random utterances with 0.25 to 0.9 s
/// pauses, as the baseline built its far-end and near-end tracks.
pub(crate) fn speech_track(pool: u8, rng: &mut Rng, len: usize) -> Vec<f32> {
    let utts: Vec<&Utterance> = corpus().iter().filter(|u| u.pool == pool).collect();
    assert!(utts.len() >= 4, "pool {pool} has {} utterances", utts.len());
    let mut out = vec![0.0f32; len];
    let mut pos = secs(rng.range(0.05, 0.3));
    while pos < len {
        let u = utts[rng.below(utts.len())];
        let n = u.pcm.len().min(len - pos);
        out[pos..pos + n].copy_from_slice(&u.pcm[..n]);
        pos += n + secs(rng.range(0.25, 0.9));
    }
    out
}

// ---------------------------------------------------------------------------
// Synthetic speech
// ---------------------------------------------------------------------------

/// A synthetic talker: pitch range and vocal-tract scale.
#[derive(Clone, Copy)]
pub(crate) struct Talker {
    pub(crate) f0_lo: f64,
    pub(crate) f0_hi: f64,
    pub(crate) tract: f64,
}

pub(crate) const TALKERS: [Talker; 4] = [
    Talker { f0_lo: 95.0, f0_hi: 140.0, tract: 1.0 },
    Talker { f0_lo: 180.0, f0_hi: 260.0, tract: 1.17 },
    Talker { f0_lo: 110.0, f0_hi: 165.0, tract: 1.05 },
    Talker { f0_lo: 200.0, f0_hi: 290.0, tract: 1.22 },
];

/// Vowel formants (F1, F2, F3) in Hz for an adult male tract.
const VOWELS: [[f64; 3]; 6] = [
    [730.0, 1090.0, 2440.0],
    [270.0, 2290.0, 3010.0],
    [300.0, 870.0, 2240.0],
    [530.0, 1840.0, 2480.0],
    [570.0, 840.0, 2410.0],
    [660.0, 1720.0, 2410.0],
];

/// Two-pole resonator with per-sample frequency.
struct Resonator {
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn process(&mut self, x: f64, f: f64, bw: f64) -> f64 {
        let r = (-std::f64::consts::PI * bw / FS as f64).exp();
        let a1 = 2.0 * r * (2.0 * std::f64::consts::PI * f / FS as f64).cos();
        let a2 = -r * r;
        let y = (1.0 - r) * x + a1 * self.y1 + a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// One utterance of `len` samples: syllables of voiced formant speech, some
/// with a fricative onset, with a falling pitch contour.
fn utterance(t: &Talker, rng: &mut Rng, len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let mut pos = 0usize;
    let mut phase = 0.0f64;
    let mut glottal = [0.0f64; 2];
    let mut res = [
        Resonator { y1: 0.0, y2: 0.0 },
        Resonator { y1: 0.0, y2: 0.0 },
        Resonator { y1: 0.0, y2: 0.0 },
    ];
    let mut last_rad = 0.0f64;
    let f0_start = rng.range(t.f0_lo + 0.4 * (t.f0_hi - t.f0_lo), t.f0_hi);
    let f0_end = rng.range(t.f0_lo, t.f0_lo + 0.3 * (t.f0_hi - t.f0_lo));
    let mut v_from = VOWELS[rng.below(VOWELS.len())];
    while pos < len {
        let syl = secs(rng.range(0.12, 0.30)).min(len - pos);
        // Optional fricative onset.
        let fric = if rng.uniform() < 0.4 { secs(rng.range(0.05, 0.11)).min(syl / 2) } else { 0 };
        let mut bp = Biquad::bandpass(rng.range(2500.0, 6000.0), 1.2);
        for i in 0..fric {
            let env = (std::f64::consts::PI * i as f64 / fric as f64).sin();
            out[pos + i] += (0.25 * env * bp.process(rng.gauss())) as f32;
        }
        let v_to = VOWELS[rng.below(VOWELS.len())];
        let voiced = syl - fric;
        let attack = secs(0.03).min(voiced / 2).max(1);
        for i in 0..voiced {
            let n = pos + fric + i;
            let prog = n as f64 / len as f64;
            let f0 = (f0_start + (f0_end - f0_start) * prog)
                * (1.0 + 0.02 * (2.0 * std::f64::consts::PI * 5.0 * n as f64 / FS as f64).sin());
            phase += f0 / FS as f64;
            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            // Glottal low-pass (two one-pole stages) and lip radiation.
            glottal[0] += 0.12 * (pulse - glottal[0]);
            glottal[1] += 0.12 * (glottal[0] - glottal[1]);
            let rad = glottal[1] - last_rad;
            last_rad = glottal[1];
            let src = rad + 0.002 * rng.gauss();
            let k = i as f64 / voiced as f64;
            let mut y = src;
            for (j, r) in res.iter_mut().enumerate() {
                let f = (v_from[j] + (v_to[j] - v_from[j]) * k) * t.tract;
                y = r.process(y, f.min(FS as f64 * 0.45), 60.0 + 40.0 * j as f64) * 4.0;
            }
            let env = if i < attack {
                i as f64 / attack as f64
            } else if i > voiced - attack {
                (voiced - i) as f64 / attack as f64
            } else {
                1.0
            };
            out[n] += (y * env) as f32;
        }
        v_from = v_to;
        pos += syl;
        // Short gap between some syllables.
        if rng.uniform() < 0.3 {
            pos += secs(rng.range(0.02, 0.06));
        }
    }
    out
}

/// A talker track: utterances of 1.2 to 3.5 s with 0.25 to 0.9 s pauses,
/// active level -26 dBFS.
pub(crate) fn talker_track(t: &Talker, rng: &mut Rng, len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let mut pos = secs(rng.range(0.05, 0.3));
    while pos < len {
        let n = secs(rng.range(1.2, 3.5)).min(len - pos);
        let u = utterance(t, rng, n);
        out[pos..pos + n].copy_from_slice(&u);
        pos += n + secs(rng.range(0.25, 0.9));
    }
    out = Biquad::highpass(80.0, 0.707).run(&out);
    set_active_level(&mut out, -26.0);
    out
}

// ---------------------------------------------------------------------------
// Rooms and the laptop echo path (image method), as in the baseline
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub(crate) struct Room {
    pub(crate) name: &'static str,
    pub(crate) dims: [f64; 3],
    pub(crate) rt60: f64,
}

pub(crate) const ROOMS: [Room; 3] = [
    Room { name: "office RT60 0.25s", dims: [3.5, 3.0, 2.5], rt60: 0.25 },
    Room { name: "living RT60 0.45s", dims: [5.0, 4.0, 2.7], rt60: 0.45 },
    Room { name: "meeting RT60 0.60s", dims: [7.0, 5.0, 3.0], rt60: 0.60 },
];

const C: f64 = 343.0;
const HALF_TAPS: isize = 32;

pub(crate) fn rir_len(room: &Room) -> usize {
    (room.rt60 * 1.1 * FS as f64) as usize
}

/// Allen & Berkley image-method RIR, high-passed at 60 Hz.
pub(crate) fn image_rir(room: &Room, src: [f64; 3], mic: [f64; 3], len: usize) -> Vec<f32> {
    let [lx, ly, lz] = room.dims;
    let v = lx * ly * lz;
    let s = 2.0 * (lx * ly + lx * lz + ly * lz);
    let alpha = (0.161 * v / (s * room.rt60)).clamp(0.01, 0.99);
    let beta = (1.0 - alpha).sqrt();
    let fs = FS as f64;
    let max_dist = len as f64 / fs * C;
    let nx = (max_dist / (2.0 * lx)).ceil() as i64 + 1;
    let ny = (max_dist / (2.0 * ly)).ceil() as i64 + 1;
    let nz = (max_dist / (2.0 * lz)).ceil() as i64 + 1;
    let mut h = vec![0.0f64; len];
    for ix in -nx..=nx {
        for px in 0..2i64 {
            let xi = if px == 0 { src[0] } else { -src[0] } + 2.0 * ix as f64 * lx;
            let dx = xi - mic[0];
            let rx = (ix - px).abs() + ix.abs();
            for iy in -ny..=ny {
                for py in 0..2i64 {
                    let yi = if py == 0 { src[1] } else { -src[1] } + 2.0 * iy as f64 * ly;
                    let dy = yi - mic[1];
                    let ry = (iy - py).abs() + iy.abs();
                    let dxy2 = dx * dx + dy * dy;
                    if dxy2.sqrt() > max_dist {
                        continue;
                    }
                    for iz in -nz..=nz {
                        for pz in 0..2i64 {
                            let zi = if pz == 0 { src[2] } else { -src[2] } + 2.0 * iz as f64 * lz;
                            let dz = zi - mic[2];
                            let dist = (dxy2 + dz * dz).sqrt().max(0.01);
                            let t = dist / C * fs;
                            if t >= (len as f64) - HALF_TAPS as f64 - 1.0 {
                                continue;
                            }
                            let rz = (iz - pz).abs() + iz.abs();
                            let amp = beta.powi((rx + ry + rz) as i32)
                                / (4.0 * std::f64::consts::PI * dist);
                            let t0 = t.floor() as isize;
                            let frac = t - t0 as f64;
                            for k in -HALF_TAPS + 1..=HALF_TAPS {
                                let idx = t0 + k;
                                if idx < 0 || idx as usize >= len {
                                    continue;
                                }
                                let x = k as f64 - frac;
                                let w = 0.5
                                    + 0.5 * (std::f64::consts::PI * x / HALF_TAPS as f64).cos();
                                let sn = if x.abs() < 1e-12 {
                                    1.0
                                } else {
                                    (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                                };
                                h[idx as usize] += amp * w * sn;
                            }
                        }
                    }
                }
            }
        }
    }
    let h: Vec<f32> = h.into_iter().map(|v| v as f32).collect();
    Biquad::highpass(60.0, 0.707).run(&h)
}

pub(crate) struct LaptopPose {
    pub(crate) spk: [f64; 3],
    pub(crate) mic: [f64; 3],
    pub(crate) talker: [f64; 3],
}

pub(crate) fn laptop_pose(room: &Room, rng: &mut Rng) -> LaptopPose {
    let [lx, ly, _] = room.dims;
    let base = [rng.range(0.3, 0.7) * lx, rng.range(0.3, 0.7) * ly, 0.75];
    let yaw = rng.range(0.0, 2.0 * std::f64::consts::PI);
    let (c, s) = (yaw.cos(), yaw.sin());
    let rot = |dx: f64, dy: f64, z: f64| [base[0] + c * dx - s * dy, base[1] + s * dx + c * dy, z];
    LaptopPose {
        spk: rot(0.0, 0.05, 0.76),
        mic: rot(0.0, -0.18, 0.98),
        talker: rot(0.0, 0.55, 1.15),
    }
}

/// Short chassis-borne coupling path.
pub(crate) fn chassis_ir(rng: &mut Rng) -> Vec<f32> {
    let n = (0.004 * FS as f64) as usize;
    let mut lp = Biquad::lowpass(3000.0, 0.7);
    let mut h: Vec<f32> = (0..n)
        .map(|i| {
            let env = (-(i as f64) / (0.0008 * FS as f64)).exp();
            let pre = if i < 10 { 0.0 } else { 1.0 };
            (lp.process(rng.gauss()) * env * pre) as f32
        })
        .collect();
    let e = energy(&h).sqrt();
    for v in h.iter_mut() {
        *v = (*v as f64 / e) as f32;
    }
    h
}

/// Laptop loudspeaker: 280 Hz high-pass driven into tanh soft clipping.
pub(crate) fn laptop_speaker(x: &[f32], drive: f64) -> Vec<f32> {
    let peak = x.iter().fold(0.0f32, |m, &v| m.max(v.abs())).max(1e-9) as f64;
    let g = 0.9 / peak;
    let hp = Biquad::highpass(280.0, 0.707).run(x);
    hp.iter().map(|&v| ((drive * g * v as f64).tanh() / drive) as f32).collect()
}

pub(crate) struct EchoPath {
    pub(crate) rir: Vec<f32>,
    pub(crate) chassis: Vec<f32>,
}

pub(crate) fn echo_path(room: &Room, pose: &LaptopPose, rng: &mut Rng) -> EchoPath {
    EchoPath {
        rir: image_rir(room, pose.spk, pose.mic, rir_len(room)),
        chassis: chassis_ir(rng),
    }
}

/// Speaker model -> room + chassis -> system delay -> active level.
pub(crate) fn make_echo(far: &[f32], p: &EchoPath, sys_delay: usize, level_db: f64) -> Vec<f32> {
    let spk = laptop_speaker(far, 1.6);
    let mut a = fft_convolve(&spk, &p.rir);
    let c = fft_convolve(&spk, &p.chassis);
    let g = (energy(&a) / energy(&c).max(1e-20)).sqrt() * 10f64.powf(-12.0 / 20.0);
    for (x, y) in a.iter_mut().zip(c.iter()) {
        *x += (g * *y as f64) as f32;
    }
    let mut e = delay(&a, sys_delay);
    set_active_level(&mut e, level_db);
    e
}

pub(crate) fn near_at_mic(near: &[f32], room: &Room, pose: &LaptopPose, level_db: f64) -> Vec<f32> {
    let h = image_rir(room, pose.talker, pose.mic, rir_len(room));
    let mut y = fft_convolve(near, &h);
    set_active_level(&mut y, level_db);
    y
}

pub(crate) fn floor_noise(rng: &mut Rng, n: usize, level_db: f64) -> Vec<f32> {
    let mut w = white(rng, n);
    scale(&mut w, 10f64.powf(level_db / 20.0));
    w
}

// ---------------------------------------------------------------------------
// Metrics (as in the baseline)
// ---------------------------------------------------------------------------

/// Per-10 ms activity of a clean reference (p95 - 25 dB, above -65 dBFS).
pub(crate) fn active_frames(x: &[f32]) -> Vec<bool> {
    let p: Vec<f64> = x.chunks(FRAME).map(|c| energy(c) / c.len() as f64).collect();
    let mut s = p.clone();
    s.sort_by(|a, b| a.total_cmp(b));
    let p95 = s[(s.len() * 95 / 100).min(s.len() - 1)];
    let th = (p95 * 10f64.powf(-2.5)).max(10f64.powf(-6.5));
    p.iter().map(|&v| v > th).collect()
}

fn frame_energies(x: &[f32]) -> Vec<f64> {
    x.chunks(FRAME).map(energy).collect()
}

/// ERLE (dB) over frames in [f0, f1) where `mask` is true; None if fewer
/// than 20 frames qualify.
pub(crate) fn erle(mic: &[f32], out: &[f32], mask: &[bool], f0: usize, f1: usize) -> Option<f64> {
    let em = frame_energies(mic);
    let eo = frame_energies(out);
    let (mut a, mut b, mut n) = (0.0, 0.0, 0);
    for f in f0..f1.min(mask.len()).min(em.len()).min(eo.len()) {
        if mask[f] {
            a += em[f];
            b += eo[f];
            n += 1;
        }
    }
    (n >= 20).then(|| db(a) - db(b))
}

/// Seconds after frame `f0` until ERLE over the next 1 s (masked frames)
/// first reaches `thr` dB.
pub(crate) fn convergence_s(
    mic: &[f32],
    out: &[f32],
    mask: &[bool],
    f0: usize,
    f_end: usize,
    thr: f64,
) -> Option<f64> {
    let em = frame_energies(mic);
    let eo = frame_energies(out);
    let w = 100;
    let end = f_end.min(mask.len()).min(em.len()).min(eo.len());
    let mut f = f0;
    while f + w <= end {
        let (mut a, mut b, mut n) = (0.0, 0.0, 0);
        for g in f..f + w {
            if mask[g] {
                a += em[g];
                b += eo[g];
                n += 1;
            }
        }
        if n >= 30 && db(a) - db(b) >= thr {
            return Some((f - f0) as f64 * 0.01);
        }
        f += 5;
    }
    None
}

/// Mean level (dBFS) of `x` on frames where `mask` is true in [f0, f1).
pub(crate) fn level_on(x: &[f32], mask: &[bool], f0: usize, f1: usize) -> f64 {
    let e = frame_energies(x);
    let (mut a, mut n) = (0.0, 0usize);
    for f in f0..f1.min(mask.len()).min(e.len()) {
        if mask[f] {
            a += e[f];
            n += FRAME;
        }
    }
    db(a / n.max(1) as f64)
}

pub(crate) fn mask_and_not(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b.iter()).map(|(x, y)| *x && !*y).collect()
}

pub(crate) fn mask_and(a: &[bool], b: &[bool]) -> Vec<bool> {
    a.iter().zip(b.iter()).map(|(x, y)| *x && *y).collect()
}

pub(crate) fn mask_not(a: &[bool]) -> Vec<bool> {
    a.iter().map(|x| !*x).collect()
}

/// Shift a frame mask later by `d` samples (rounded to frames).
pub(crate) fn shift_mask(m: &[bool], d: usize) -> Vec<bool> {
    let k = (d + FRAME / 2) / FRAME;
    (0..m.len()).map(|i| i >= k && m[i - k]).collect()
}

pub(crate) fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

pub(crate) fn min(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::INFINITY, f64::min)
}

pub(crate) fn max(v: &[f64]) -> f64 {
    v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
}
