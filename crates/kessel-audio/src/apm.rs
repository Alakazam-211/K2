//! Echo cancellation, noise suppression and gain control (sonora, a pure
//! Rust port of WebRTC's audio processing module), and the caller's
//! capture stage between them.
//!
//! The playback mix is the echo reference: every 10 ms block the engine
//! plays is passed to [`Apm::render`] before it reaches the device.

use crate::error::AudioError;
use crate::frame::{BLOCK_SAMPLES, SAMPLE_RATE};
use crate::stage::{CaptureStage, CaptureStageStats, StageRunner};
use sonora::config::{
    AdaptiveDigital, EchoCanceller, FixedDigital, GainController2, HighPassFilter,
    MaxProcessingRate, NoiseSuppression, NoiseSuppressionLevel, Pipeline,
};

/// Noise-suppression strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoiseSuppressionStrength {
    /// About 6 dB.
    Low,
    /// About 12 dB (the default).
    Moderate,
    /// About 18 dB.
    High,
    /// About 21 dB.
    VeryHigh,
}

/// Which processing runs on the capture path.
///
/// `#[non_exhaustive]`: start from [`ProcessingConfig::default`] or
/// [`ProcessingConfig::off`], then set fields or use the `with_` methods.
#[non_exhaustive]
pub struct ProcessingConfig {
    /// Acoustic echo cancellation (AEC3) against the playback mix.
    pub echo_cancellation: bool,
    /// Noise suppression, or `None` for off. Turn it off when a
    /// [`capture_stage`](Self::capture_stage) removes noise: two
    /// suppressors in a row add artefacts for no gain. The engine doesn't
    /// decide that for you.
    pub noise_suppression: Option<NoiseSuppressionStrength>,
    /// Automatic gain control (AGC2 adaptive digital, plus its limiter).
    pub gain_control: bool,
    /// The high-pass filter (removes DC and rumble below about 100 Hz).
    /// Echo cancellation turns it on regardless.
    pub high_pass: bool,
    /// The caller's own processing, after the high-pass filter, echo
    /// cancellation and noise suppression (if set) and before gain control.
    /// See [`CaptureStage`] for the rules, the time budget and the bypass.
    /// Levels, VAD and DTX read the signal after the stage and gain control.
    ///
    /// `None` (the default) runs exactly what 0.1 ran. With a stage, echo
    /// cancellation and gain control run in two sonora instances with the
    /// stage between them.
    pub capture_stage: Option<Box<dyn CaptureStage>>,
    /// The step-down floor: sonora's noise suppression at this strength
    /// stands in while the capture stage is bypassed (over budget or a
    /// fault), and stops again when a retry brings the stage back. The
    /// switch crossfades over one 10 ms block and keeps the stage's timing.
    /// Default `Some(Moderate)`, as 0.1 runs it.
    ///
    /// It only applies with a stage set and
    /// [`noise_suppression`](Self::noise_suppression) `None` (otherwise
    /// noise suppression already runs before the stage). The suppressor
    /// runs on every block in its own sonora instance, so it is converged
    /// the moment it is needed; it allocates nothing per block, and costs
    /// its band split and merge on top of the suppression 0.1 ran: about
    /// 0.5% of a core (50–54 µs per 10 ms block, measured on x86-64). Its own delay (334 samples,
    /// 7 ms) is taken out of the stage's latency, so the timing holds for a
    /// stage with at least that much latency (a shorter one: the suppressed
    /// signal is that much later). `None` turns the floor off: a bypassed
    /// stage leaves no suppression.
    pub stage_fallback_noise_suppression: Option<NoiseSuppressionStrength>,
}

impl std::fmt::Debug for ProcessingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessingConfig")
            .field("echo_cancellation", &self.echo_cancellation)
            .field("noise_suppression", &self.noise_suppression)
            .field("gain_control", &self.gain_control)
            .field("high_pass", &self.high_pass)
            .field(
                "capture_stage_latency",
                &self.capture_stage.as_ref().map(|s| s.latency_samples()),
            )
            .field(
                "stage_fallback_noise_suppression",
                &self.stage_fallback_noise_suppression,
            )
            .finish()
    }
}

impl Default for ProcessingConfig {
    /// Everything on: AEC3, moderate noise suppression, AGC2 and the
    /// high-pass filter. No capture stage.
    fn default() -> Self {
        ProcessingConfig {
            echo_cancellation: true,
            noise_suppression: Some(NoiseSuppressionStrength::Moderate),
            gain_control: true,
            high_pass: true,
            capture_stage: None,
            stage_fallback_noise_suppression: Some(NoiseSuppressionStrength::Moderate),
        }
    }
}

impl ProcessingConfig {
    /// No processing at all: capture passes through unchanged.
    pub fn off() -> Self {
        ProcessingConfig {
            echo_cancellation: false,
            noise_suppression: None,
            gain_control: false,
            high_pass: false,
            capture_stage: None,
            stage_fallback_noise_suppression: Some(NoiseSuppressionStrength::Moderate),
        }
    }

    /// Set the [`capture_stage`](Self::capture_stage). Noise suppression
    /// stays as it is: turn it off with
    /// [`with_noise_suppression(None)`](Self::with_noise_suppression) if the
    /// stage removes noise.
    pub fn with_capture_stage(mut self, stage: Box<dyn CaptureStage>) -> Self {
        self.capture_stage = Some(stage);
        self
    }

    /// Set [`noise_suppression`](Self::noise_suppression).
    pub fn with_noise_suppression(mut self, ns: Option<NoiseSuppressionStrength>) -> Self {
        self.noise_suppression = ns;
        self
    }

    /// Set [`echo_cancellation`](Self::echo_cancellation).
    pub fn with_echo_cancellation(mut self, on: bool) -> Self {
        self.echo_cancellation = on;
        self
    }

    /// Set [`gain_control`](Self::gain_control).
    pub fn with_gain_control(mut self, on: bool) -> Self {
        self.gain_control = on;
        self
    }

    /// Set [`high_pass`](Self::high_pass).
    pub fn with_high_pass(mut self, on: bool) -> Self {
        self.high_pass = on;
        self
    }

    /// Set
    /// [`stage_fallback_noise_suppression`](Self::stage_fallback_noise_suppression).
    pub fn with_stage_fallback_noise_suppression(
        mut self,
        ns: Option<NoiseSuppressionStrength>,
    ) -> Self {
        self.stage_fallback_noise_suppression = ns;
        self
    }

    /// What sonora runs: everything but the stage.
    pub(crate) fn settings(&self) -> ApmSettings {
        ApmSettings {
            echo_cancellation: self.echo_cancellation,
            noise_suppression: self.noise_suppression,
            gain_control: self.gain_control,
            high_pass: self.high_pass,
            stage_fallback_ns: self.stage_fallback_noise_suppression,
        }
    }
}

/// [`ProcessingConfig`] without the stage: what sonora runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ApmSettings {
    pub(crate) echo_cancellation: bool,
    pub(crate) noise_suppression: Option<NoiseSuppressionStrength>,
    pub(crate) gain_control: bool,
    pub(crate) high_pass: bool,
    /// Only used with a stage (see
    /// [`ProcessingConfig::stage_fallback_noise_suppression`]).
    pub(crate) stage_fallback_ns: Option<NoiseSuppressionStrength>,
}

impl ApmSettings {
    fn any(&self) -> bool {
        self.echo_cancellation
            || self.noise_suppression.is_some()
            || self.gain_control
            || self.high_pass
    }
}

impl From<&ProcessingConfig> for ApmSettings {
    fn from(c: &ProcessingConfig) -> Self {
        c.settings()
    }
}

impl From<ProcessingConfig> for ApmSettings {
    fn from(c: ProcessingConfig) -> Self {
        c.settings()
    }
}

/// AGC2 adaptive-digital headroom, in dB.
///
/// With WebRTC's default of 5 dB, sonora 0.2.0 settles 0.7 to 1.6 dB
/// quieter than the C++ WebRTC library. Measured on LibriSpeech speech
/// against the C++ library (5.0 / 4.0 / 3.4 / 3.0 dB swept), 4 dB matches
/// it best: AGC2 alone at -40 and -30 dBFS input lands at -22.8 and
/// -23.0 dBFS (C++: -23.1, -23.1); the full chain on near-end speech at
/// -23.5 dBFS (C++: -22.9) and in double talk at -25.5 dBFS (C++: -25.4).
/// Every level is within 0.6 dB of the C++ library, against up to 1.6 dB
/// with 5 dB. Quiet (-50 dBFS) and loud (-20 dBFS) input are unaffected.
pub const AGC2_HEADROOM_DB: f32 = 4.0;

/// One sonora instance for `cfg`, or `None` if it would do nothing.
fn build(cfg: &ApmSettings, headroom_db: f32) -> Option<sonora::AudioProcessing> {
    if !cfg.any() {
        return None;
    }
    let config = sonora::Config {
        pipeline: Pipeline {
            maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
            multi_channel_render: false,
            multi_channel_capture: false,
            ..Pipeline::default()
        },
        high_pass_filter: (cfg.high_pass || cfg.echo_cancellation).then(HighPassFilter::default),
        echo_canceller: cfg.echo_cancellation.then(EchoCanceller::default),
        noise_suppression: cfg.noise_suppression.map(|s| NoiseSuppression {
            level: match s {
                NoiseSuppressionStrength::Low => NoiseSuppressionLevel::Low,
                NoiseSuppressionStrength::Moderate => NoiseSuppressionLevel::Moderate,
                NoiseSuppressionStrength::High => NoiseSuppressionLevel::High,
                NoiseSuppressionStrength::VeryHigh => NoiseSuppressionLevel::VeryHigh,
            },
            analyze_linear_aec_output_when_available: false,
        }),
        gain_controller2: cfg.gain_control.then(|| GainController2 {
            input_volume_controller: false,
            adaptive_digital: Some(AdaptiveDigital {
                headroom_db,
                ..AdaptiveDigital::default()
            }),
            fixed_digital: FixedDigital::default(),
        }),
        ..Default::default()
    };
    Some(
        sonora::AudioProcessing::builder()
            .config(config)
            .capture_config(sonora::StreamConfig::new(SAMPLE_RATE, 1))
            .render_config(sonora::StreamConfig::new(SAMPLE_RATE, 1))
            .build(),
    )
}

/// One capture block through `apm`, or a copy without one.
fn run_capture(apm: Option<&mut sonora::AudioProcessing>, x: &[f32], out: &mut [f32]) {
    match apm {
        Some(apm) => {
            // Frame-size mismatches are impossible here (fixed 480-sample
            // mono blocks), so an error would be a bug in the caller.
            if apm.process_capture_f32(&[x], &mut [out]).is_err() {
                debug_assert!(false, "sonora rejected a capture block");
                out.copy_from_slice(x);
            }
        }
        None => out.copy_from_slice(x),
    }
}

/// The algorithmic delay of a noise-suppression-only sonora instance at
/// 48 kHz, in samples (7 ms): its band split and merge (46) plus the
/// suppressor's overlap (288). Measured (`the_step_down_lines_up_with_the_stage`).
pub(crate) const STEP_DOWN_NS_DELAY: usize = 334;

/// The step-down floor: sonora's noise suppression, run on every block on
/// the echo canceller's output and delayed to the stage's latency, used
/// while the stage is bypassed.
struct StepDown {
    ns: sonora::AudioProcessing,
    /// This block, suppressed and delayed.
    out: [f32; BLOCK_SAMPLES],
    /// Tops the suppressor's own delay up to the stage's latency.
    delay: Box<[f32]>,
    pos: usize,
    /// The suppressor's output was used for the last block.
    on: bool,
    /// The crossfade, 0 to 1 over a block (raised cosine).
    ramp: [f32; BLOCK_SAMPLES],
}

impl StepDown {
    fn new(strength: NoiseSuppressionStrength, stage_latency: usize) -> Option<StepDown> {
        let ns = build(
            &ApmSettings {
                echo_cancellation: false,
                noise_suppression: Some(strength),
                gain_control: false,
                high_pass: false,
                stage_fallback_ns: None,
            },
            AGC2_HEADROOM_DB,
        )?;
        let mut ramp = [0.0f32; BLOCK_SAMPLES];
        for (i, r) in ramp.iter_mut().enumerate() {
            let t = (i as f32 + 0.5) / BLOCK_SAMPLES as f32;
            *r = 0.5 - 0.5 * (std::f32::consts::PI * t).cos();
        }
        Some(StepDown {
            ns,
            out: [0.0; BLOCK_SAMPLES],
            delay: vec![0.0; stage_latency.saturating_sub(STEP_DOWN_NS_DELAY)].into_boxed_slice(),
            pos: 0,
            on: false,
            ramp,
        })
    }

    /// Suppress this block of the echo canceller's output into `self.out`,
    /// delayed to the stage's latency.
    fn feed(&mut self, aec: &[f32; BLOCK_SAMPLES]) {
        run_capture(Some(&mut self.ns), aec, &mut self.out);
        let l = self.delay.len();
        if l == 0 {
            return;
        }
        for v in self.out.iter_mut() {
            std::mem::swap(&mut self.delay[self.pos], v);
            self.pos += 1;
            if self.pos == l {
                self.pos = 0;
            }
        }
    }

    /// `block` holds the stage's output (or the delayed echo canceller
    /// output when bypassed); replace it with the suppressor's when
    /// `use_ns`, crossfading over this block when that changes.
    fn apply(&mut self, block: &mut [f32; BLOCK_SAMPLES], use_ns: bool) {
        if use_ns != self.on {
            for ((b, &n), &r) in block.iter_mut().zip(&self.out).zip(&self.ramp) {
                let (from, to) = if use_ns { (*b, n) } else { (n, *b) };
                *b = from + (to - from) * r;
            }
            self.on = use_ns;
        } else if use_ns {
            block.copy_from_slice(&self.out);
        }
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.pos = 0;
    }
}

pub(crate) struct Apm {
    /// Without a stage, everything in one instance (as 0.1 ran it). With
    /// one: the high-pass filter, echo cancellation and noise suppression.
    inner: Option<sonora::AudioProcessing>,
    /// With a stage and gain control: AGC2 and its limiter, alone.
    post: Option<sonora::AudioProcessing>,
    stage: Option<StageRunner>,
    /// With a stage, noise suppression off and the fallback on.
    step_down: Option<StepDown>,
    scratch: [f32; BLOCK_SAMPLES],
    mid: [f32; BLOCK_SAMPLES],
}

impl Apm {
    pub(crate) fn new(cfg: impl Into<ApmSettings>) -> Self {
        Self::with_agc_headroom(cfg, AGC2_HEADROOM_DB)
    }

    pub(crate) fn with_agc_headroom(cfg: impl Into<ApmSettings>, headroom_db: f32) -> Self {
        Apm {
            inner: build(&cfg.into(), headroom_db),
            post: None,
            stage: None,
            step_down: None,
            scratch: [0.0; BLOCK_SAMPLES],
            mid: [0.0; BLOCK_SAMPLES],
        }
    }

    /// With a capture stage: the high-pass filter, echo cancellation and
    /// noise suppression in one instance, then the stage, then gain control
    /// in a second instance; and the step-down suppressor if it applies.
    /// Build it on the thread that will run it (the stage is reset here).
    pub(crate) fn with_stage(
        cfg: impl Into<ApmSettings>,
        stage: Box<dyn CaptureStage>,
    ) -> Result<Self, AudioError> {
        Self::with_runner(cfg.into(), StageRunner::new(stage)?)
    }

    /// [`Apm::with_stage`] with the stage timed by `clock` (tests).
    #[cfg(test)]
    pub(crate) fn with_stage_clock(
        cfg: impl Into<ApmSettings>,
        stage: Box<dyn CaptureStage>,
        clock: crate::stage::StageClock,
    ) -> Result<Self, AudioError> {
        Self::with_runner(cfg.into(), StageRunner::with_clock(stage, clock)?)
    }

    fn with_runner(cfg: ApmSettings, stage: StageRunner) -> Result<Self, AudioError> {
        let pre = ApmSettings {
            gain_control: false,
            stage_fallback_ns: None,
            ..cfg
        };
        let post = ApmSettings {
            echo_cancellation: false,
            noise_suppression: None,
            gain_control: cfg.gain_control,
            high_pass: false,
            stage_fallback_ns: None,
        };
        let step_down = match (cfg.noise_suppression, cfg.stage_fallback_ns) {
            (None, Some(strength)) => StepDown::new(strength, stage.latency()),
            _ => None,
        };
        Ok(Apm {
            inner: build(&pre, AGC2_HEADROOM_DB),
            post: build(&post, AGC2_HEADROOM_DB),
            stage: Some(stage),
            step_down,
            scratch: [0.0; BLOCK_SAMPLES],
            mid: [0.0; BLOCK_SAMPLES],
        })
    }

    /// Analyse one 10 ms block of the playback mix (the echo reference).
    pub(crate) fn render(&mut self, x: &[f32]) {
        debug_assert_eq!(x.len(), BLOCK_SAMPLES);
        if let Some(apm) = self.inner.as_mut() {
            // Frame-size mismatches are impossible here (fixed 480-sample
            // mono blocks), so an error would be a bug in the caller.
            if apm
                .process_render_f32(&[x], &mut [&mut self.scratch[..]])
                .is_err()
            {
                debug_assert!(false, "sonora rejected a render block");
            }
        }
    }

    /// Process one 10 ms capture block into `out`.
    pub(crate) fn capture(&mut self, x: &[f32], out: &mut [f32]) {
        debug_assert_eq!(x.len(), BLOCK_SAMPLES);
        debug_assert_eq!(out.len(), BLOCK_SAMPLES);
        let Some(stage) = self.stage.as_mut() else {
            run_capture(self.inner.as_mut(), x, out);
            return;
        };
        run_capture(self.inner.as_mut(), x, &mut self.mid);
        if let Some(sd) = self.step_down.as_mut() {
            sd.feed(&self.mid);
        }
        stage.process(&mut self.mid);
        if let Some(sd) = self.step_down.as_mut() {
            let use_ns = !stage.used_stage();
            sd.apply(&mut self.mid, use_ns);
            stage.set_fallback(use_ns);
        }
        run_capture(self.post.as_mut(), &self.mid, out);
    }

    /// The capture stream restarted or had a gap: reset the capture stage.
    pub(crate) fn reset_stage(&mut self) {
        if let Some(s) = self.stage.as_mut() {
            s.reset();
        }
        if let Some(sd) = self.step_down.as_mut() {
            sd.reset();
        }
    }

    /// The capture stage's counters, if there is a stage.
    pub(crate) fn stage_stats(&mut self) -> Option<CaptureStageStats> {
        self.stage.as_mut().map(|s| s.stats())
    }

    /// True once after the stage's state changed.
    pub(crate) fn take_stage_changed(&mut self) -> bool {
        self.stage.as_mut().is_some_and(|s| s.take_changed())
    }

    /// The echo canceller's delay estimate (ms).
    pub(crate) fn delay_ms(&self) -> Option<i32> {
        self.inner.as_ref().and_then(|a| a.statistics().delay_ms)
    }

    /// The echo canceller's ERLE estimate (dB).
    pub(crate) fn erle_db(&self) -> Option<f32> {
        self.inner
            .as_ref()
            .and_then(|a| a.statistics().echo_return_loss_enhancement)
            .map(|v| v as f32)
    }
}
