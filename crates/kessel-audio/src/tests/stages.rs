//! Capture stages for the tests: a pass-through (a pure 10 ms delay), a
//! scripted pass-through that can sleep, spin, panic or return NaN on cue,
//! and an RNNoise-like denoiser of about RNNoise's cost.
//!
//! No real RNNoise model is used (it would be a new dependency). The
//! RNNoise-like stage has RNNoise's shape and cost: a 960-point STFT with
//! 480-sample hops (latency 480), 22 band energies, a pitch correlation
//! search over 256 lags, a GRU stack of RNNoise's sizes (24, 48 and 96 units,
//! about 86k multiply-adds) with fixed pseudo-random weights, and band gains
//! applied before overlap-add. The weights are not trained: the gains that
//! remove noise come from a Wiener rule over a minimum-tracking noise floor,
//! and the network only nudges them (so it can't wreck the bars, but its
//! cost is real).
//!
//! In a debug build the pitch search and the GRU stack shrink (64 lags; 6,
//! 12 and 24 units): unoptimised, the full size can approach the 6 ms
//! budget on a busy machine and be bypassed, and cost only matters in the
//! CPU bars, which run optimised.

#![allow(clippy::needless_range_loop)]

use crate::frame::BLOCK_SAMPLES;
use crate::stage::CaptureStage;
use crate::testkit::Rng;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const N: usize = BLOCK_SAMPLES;

/// Copies the block out one block later: a stage that does nothing but its
/// reported 480 samples of latency.
pub(crate) struct PassThrough {
    prev: [f32; N],
}

impl PassThrough {
    pub(crate) fn new() -> Self {
        PassThrough { prev: [0.0; N] }
    }
}

impl CaptureStage for PassThrough {
    fn process(&mut self, block: &mut [f32; N]) {
        std::mem::swap(block, &mut self.prev);
    }
    fn latency_samples(&self) -> usize {
        N
    }
    fn reset(&mut self) {
        self.prev.fill(0.0);
    }
}

/// Cues for [`Scripted`]; the test holds a clone.
#[derive(Default)]
pub(crate) struct Script {
    /// `process` calls so far.
    pub(crate) calls: AtomicU64,
    /// `reset` calls so far.
    pub(crate) resets: AtomicU64,
    /// Sleep this long (µs) in every call while non-zero.
    pub(crate) sleep_us: AtomicU64,
    /// Busy-wait `spin_us` on every call whose number (from 1) is a
    /// multiple of this (0: never).
    pub(crate) spin_every: AtomicU64,
    pub(crate) spin_us: AtomicU64,
    /// Panic on this call number (0: never).
    pub(crate) panic_on: AtomicU64,
    /// Return a NaN on this call number (0: never).
    pub(crate) nan_on: AtomicU64,
    /// Panic in `reset`.
    pub(crate) panic_on_reset: AtomicBool,
}

/// A [`PassThrough`] that follows a [`Script`].
pub(crate) struct Scripted {
    inner: PassThrough,
    script: Arc<Script>,
}

impl Scripted {
    pub(crate) fn new() -> (Self, Arc<Script>) {
        let script = Arc::new(Script::default());
        (
            Scripted {
                inner: PassThrough::new(),
                script: script.clone(),
            },
            script,
        )
    }
}

impl CaptureStage for Scripted {
    fn process(&mut self, block: &mut [f32; N]) {
        let s = &self.script;
        let n = s.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if s.panic_on.load(Ordering::Relaxed) == n {
            panic!("scripted stage panic on call {n}");
        }
        let sleep = s.sleep_us.load(Ordering::Relaxed);
        if sleep > 0 {
            std::thread::sleep(Duration::from_micros(sleep));
        }
        let every = s.spin_every.load(Ordering::Relaxed);
        if every > 0 && n.is_multiple_of(every) {
            let until = Instant::now() + Duration::from_micros(s.spin_us.load(Ordering::Relaxed));
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
        self.inner.process(block);
        if s.nan_on.load(Ordering::Relaxed) == n {
            block[7] = f32::NAN;
        }
    }
    fn latency_samples(&self) -> usize {
        N
    }
    fn reset(&mut self) {
        self.script.resets.fetch_add(1, Ordering::Relaxed);
        if self.script.panic_on_reset.load(Ordering::Relaxed) {
            panic!("scripted stage panic in reset");
        }
        self.inner.reset();
    }
}

const WIN: usize = 2 * N;
const BINS: usize = N + 1;
/// RNNoise's band edges, in 200 Hz units (4 bins of 50 Hz).
const EDGES: [usize; 22] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 34, 40, 48, 60, 78, 100,
];
const BANDS: usize = 22;
const FEATURES: usize = 42;
/// Pitch search: the correlation length and lags, at 24 kHz.
const PITCH_LEN: usize = 384;
/// 1 optimised; 4 in a debug build (see the module notes).
const SHRINK: usize = if cfg!(debug_assertions) { 4 } else { 1 };
const PITCH_LAGS: usize = 256 / SHRINK;
/// The GRU stack: RNNoise's 24, 48 and 96 units.
const U1: usize = 24 / SHRINK;
const U2: usize = 48 / SHRINK;
const U3: usize = 96 / SHRINK;
const PITCH_MIN_LAG: usize = 32;
const HIST: usize = PITCH_LEN + PITCH_MIN_LAG + PITCH_LAGS;

/// One GRU layer (update, reset and candidate weights for input and state).
struct Gru {
    inputs: usize,
    units: usize,
    w: Vec<f32>,
    state: Vec<f32>,
    z: Vec<f32>,
    r: Vec<f32>,
    rh: Vec<f32>,
}

impl Gru {
    fn new(inputs: usize, units: usize, rng: &mut Rng) -> Gru {
        let scale = 1.0 / ((inputs + units) as f64).sqrt();
        Gru {
            inputs,
            units,
            w: (0..3 * units * (inputs + units))
                .map(|_| (rng.range(-1.0, 1.0) * scale) as f32)
                .collect(),
            state: vec![0.0; units],
            z: vec![0.0; units],
            r: vec![0.0; units],
            rh: vec![0.0; units],
        }
    }

    fn row(&self, gate: usize, u: usize, x: &[f32], h: &[f32]) -> f32 {
        let n = self.inputs + self.units;
        let w = &self.w[(gate * self.units + u) * n..(gate * self.units + u + 1) * n];
        let mut acc = 0.0f32;
        for (a, b) in w[..self.inputs].iter().zip(x) {
            acc += a * b;
        }
        for (a, b) in w[self.inputs..].iter().zip(h) {
            acc += a * b;
        }
        acc
    }

    fn step(&mut self, x: &[f32]) {
        debug_assert_eq!(x.len(), self.inputs);
        for u in 0..self.units {
            self.z[u] = sigmoid(self.row(0, u, x, &self.state));
            self.r[u] = sigmoid(self.row(1, u, x, &self.state));
        }
        for u in 0..self.units {
            self.rh[u] = self.r[u] * self.state[u];
        }
        for u in 0..self.units {
            let c = self.row(2, u, x, &self.rh).tanh();
            self.state[u] = (1.0 - self.z[u]) * self.state[u] + self.z[u] * c;
        }
    }

    fn reset(&mut self) {
        self.state.fill(0.0);
    }
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// A denoiser with RNNoise's shape and cost (see the module notes).
pub(crate) struct RnnLike {
    /// Planned in the first `reset` (it allocates), as a real stage may do:
    /// `process` before any `reset` panics.
    fft: Option<Arc<dyn Fft<f32>>>,
    ifft: Option<Arc<dyn Fft<f32>>>,
    scratch: Vec<Complex<f32>>,
    spec: Vec<Complex<f32>>,
    window: Vec<f32>,
    prev_in: [f32; N],
    ola: [f32; N],
    energy: [f32; BANDS],
    last_log: [f32; BANDS],
    noise: [f32; BANDS],
    noise_init: bool,
    gain: [f32; BANDS],
    features: [f32; FEATURES],
    dense_w: Vec<f32>,
    dense: Vec<f32>,
    gru1: Gru,
    gru2: Gru,
    gru3: Gru,
    in2: Vec<f32>,
    in3: Vec<f32>,
    out_w: Vec<f32>,
    net: [f32; BANDS],
    hist: Vec<f32>,
    /// Each FFT bin's band.
    band: Vec<usize>,
    /// On: denoise. Off: unity gains (still the same latency and cost).
    on: Arc<AtomicBool>,
}

fn band_of(bin: usize) -> usize {
    let edge = bin / 4;
    match EDGES.iter().rposition(|&e| e <= edge) {
        Some(b) => b.min(BANDS - 1),
        None => 0,
    }
}

impl RnnLike {
    pub(crate) fn new(seed: u64) -> (Self, Arc<AtomicBool>) {
        // Vorbis power-complementary window: w[n]² + w[n + 480]² = 1.
        let window = (0..WIN)
            .map(|n| {
                let s = (std::f64::consts::PI * (n as f64 + 0.5) / WIN as f64).sin();
                (std::f64::consts::FRAC_PI_2 * s * s).sin() as f32
            })
            .collect();
        let mut rng = Rng::new(seed);
        let on = Arc::new(AtomicBool::new(true));
        let stage = RnnLike {
            fft: None,
            ifft: None,
            scratch: Vec::new(),
            spec: vec![Complex::default(); WIN],
            window,
            prev_in: [0.0; N],
            ola: [0.0; N],
            energy: [0.0; BANDS],
            last_log: [0.0; BANDS],
            noise: [0.0; BANDS],
            noise_init: false,
            gain: [1.0; BANDS],
            features: [0.0; FEATURES],
            dense_w: (0..FEATURES * U1)
                .map(|_| (rng.range(-1.0, 1.0) / (FEATURES as f64).sqrt()) as f32)
                .collect(),
            dense: vec![0.0; U1],
            gru1: Gru::new(U1, U1, &mut rng),
            gru2: Gru::new(U1 + U1 + FEATURES, U2, &mut rng),
            gru3: Gru::new(U1 + U2 + FEATURES, U3, &mut rng),
            in2: vec![0.0; U1 + U1 + FEATURES],
            in3: vec![0.0; U1 + U2 + FEATURES],
            out_w: (0..U3 * BANDS)
                .map(|_| (rng.range(-1.0, 1.0) / (U3 as f64).sqrt()) as f32)
                .collect(),
            net: [0.0; BANDS],
            hist: vec![0.0; HIST],
            band: (0..BINS).map(band_of).collect(),
            on: on.clone(),
        };
        (stage, on)
    }

    /// The best normalised pitch correlation over the history (24 kHz).
    fn pitch(&mut self, block: &[f32; N]) -> f32 {
        let fresh = N / 2;
        self.hist.copy_within(fresh.., 0);
        let tail = HIST - fresh;
        for (i, slot) in self.hist[tail..].iter_mut().enumerate() {
            *slot = 0.5 * (block[2 * i] + block[2 * i + 1]);
        }
        let x = &self.hist[HIST - PITCH_LEN..];
        let e0: f32 = x.iter().map(|v| v * v).sum::<f32>() + 1e-9;
        let mut best = 0.0f32;
        for lag in PITCH_MIN_LAG..PITCH_MIN_LAG + PITCH_LAGS {
            let y = &self.hist[HIST - PITCH_LEN - lag..HIST - lag];
            let mut c = 0.0f32;
            for (a, b) in x.iter().zip(y) {
                c += a * b;
            }
            best = best.max(c / e0);
        }
        best.clamp(-1.0, 1.0)
    }

    fn network(&mut self) {
        for (j, d) in self.dense.iter_mut().enumerate() {
            let w = &self.dense_w[j * FEATURES..(j + 1) * FEATURES];
            *d = w
                .iter()
                .zip(&self.features)
                .map(|(a, b)| a * b)
                .sum::<f32>()
                .tanh();
        }
        self.gru1.step(&self.dense);
        self.in2[..U1].copy_from_slice(&self.dense);
        self.in2[U1..2 * U1].copy_from_slice(&self.gru1.state);
        self.in2[2 * U1..].copy_from_slice(&self.features);
        self.gru2.step(&self.in2);
        self.in3[..U1].copy_from_slice(&self.dense);
        self.in3[U1..U1 + U2].copy_from_slice(&self.gru2.state);
        self.in3[U1 + U2..].copy_from_slice(&self.features);
        self.gru3.step(&self.in3);
        for (b, o) in self.net.iter_mut().enumerate() {
            let w = &self.out_w[b * U3..(b + 1) * U3];
            *o = sigmoid(w.iter().zip(&self.gru3.state).map(|(a, s)| a * s).sum());
        }
    }
}

impl CaptureStage for RnnLike {
    fn process(&mut self, block: &mut [f32; N]) {
        // Analysis: the previous and this block, windowed.
        for i in 0..N {
            self.spec[i] = Complex::new(self.prev_in[i] * self.window[i], 0.0);
            self.spec[N + i] = Complex::new(block[i] * self.window[N + i], 0.0);
        }
        self.prev_in.copy_from_slice(block);
        self.fft
            .as_ref()
            .expect("process before reset")
            .process_with_scratch(&mut self.spec, &mut self.scratch);

        self.energy.fill(1e-10);
        for k in 0..BINS {
            self.energy[self.band[k]] += self.spec[k].norm_sqr();
        }
        let pitch = self.pitch(block);
        for b in 0..BANDS {
            let l = (self.energy[b]).log10();
            self.features[b] = l;
            if b < FEATURES - BANDS {
                self.features[BANDS + b] = l - self.last_log[b];
            }
            self.last_log[b] = l;
        }
        self.features[FEATURES - 1] = pitch;
        self.network();

        // Gains: Wiener over a minimum-tracking noise floor, nudged by the
        // network. Unity when switched off.
        if !self.noise_init {
            self.noise = self.energy;
            self.noise_init = true;
        }
        let on = self.on.load(Ordering::Relaxed);
        for b in 0..BANDS {
            let e = self.energy[b];
            self.noise[b] = if e < self.noise[b] {
                0.7 * self.noise[b] + 0.3 * e
            } else {
                (self.noise[b] * 1.01).min(e)
            };
            let snr = (e / (2.0 * self.noise[b] + 1e-10) - 1.0).max(0.0);
            let wiener = (snr / (snr + 1.0)).max(0.1);
            let g = wiener.max(0.6 * self.gain[b]) * (0.95 + 0.05 * self.net[b]);
            self.gain[b] = g.clamp(0.1, 1.0);
        }
        for k in 0..BINS {
            let g = if on { self.gain[self.band[k]] } else { 1.0 };
            self.spec[k] *= g;
            if k > 0 && k < N {
                self.spec[WIN - k] = self.spec[k].conj();
            }
        }

        // Synthesis: overlap-add of the windowed inverse, one block late.
        self.ifft
            .as_ref()
            .expect("process before reset")
            .process_with_scratch(&mut self.spec, &mut self.scratch);
        let s = 1.0 / WIN as f32;
        for i in 0..N {
            block[i] = self.ola[i] + self.spec[i].re * s * self.window[i];
            self.ola[i] = self.spec[N + i].re * s * self.window[N + i];
        }
    }
    fn latency_samples(&self) -> usize {
        N
    }
    fn reset(&mut self) {
        if self.fft.is_none() {
            let mut planner = FftPlanner::<f32>::new();
            let fft = planner.plan_fft_forward(WIN);
            let ifft = planner.plan_fft_inverse(WIN);
            let len = fft
                .get_inplace_scratch_len()
                .max(ifft.get_inplace_scratch_len());
            self.scratch = vec![Complex::default(); len];
            self.fft = Some(fft);
            self.ifft = Some(ifft);
        }
        self.prev_in.fill(0.0);
        self.ola.fill(0.0);
        self.noise_init = false;
        self.gain = [1.0; BANDS];
        self.last_log = [0.0; BANDS];
        self.hist.fill(0.0);
        self.gru1.reset();
        self.gru2.reset();
        self.gru3.reset();
    }
}
