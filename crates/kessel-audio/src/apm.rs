//! Echo cancellation, noise suppression and gain control (sonora, a pure
//! Rust port of WebRTC's audio processing module).
//!
//! The playback mix is the echo reference: every 10 ms block the engine
//! plays is passed to [`Apm::render`] before it reaches the device.

use crate::frame::{BLOCK_SAMPLES, SAMPLE_RATE};
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
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessingConfig {
    /// Acoustic echo cancellation (AEC3) against the playback mix.
    pub echo_cancellation: bool,
    /// Noise suppression, or `None` for off.
    pub noise_suppression: Option<NoiseSuppressionStrength>,
    /// Automatic gain control (AGC2 adaptive digital, plus its limiter).
    pub gain_control: bool,
    /// The high-pass filter (removes DC and rumble below about 100 Hz).
    /// Echo cancellation turns it on regardless.
    pub high_pass: bool,
}

impl Default for ProcessingConfig {
    /// Everything on: AEC3, moderate noise suppression, AGC2 and the
    /// high-pass filter.
    fn default() -> Self {
        ProcessingConfig {
            echo_cancellation: true,
            noise_suppression: Some(NoiseSuppressionStrength::Moderate),
            gain_control: true,
            high_pass: true,
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
        }
    }

    fn any(&self) -> bool {
        self.echo_cancellation
            || self.noise_suppression.is_some()
            || self.gain_control
            || self.high_pass
    }
}

/// AGC2 adaptive-digital headroom, in dB. sonora 0.2.0's default (5 dB)
/// settles about 0.7 to 1.6 dB quieter than the C++ WebRTC library with the
/// same settings; this value brings it back in line (measured on LibriSpeech
/// against the C++ library, see the crate README).
pub const AGC2_HEADROOM_DB: f32 = 5.0;

pub(crate) struct Apm {
    inner: Option<sonora::AudioProcessing>,
    scratch: [f32; BLOCK_SAMPLES],
}

impl Apm {
    pub(crate) fn new(cfg: &ProcessingConfig) -> Self {
        if !cfg.any() {
            return Apm {
                inner: None,
                scratch: [0.0; BLOCK_SAMPLES],
            };
        }
        let config = sonora::Config {
            pipeline: Pipeline {
                maximum_internal_processing_rate: MaxProcessingRate::Rate48kHz,
                multi_channel_render: false,
                multi_channel_capture: false,
                ..Pipeline::default()
            },
            high_pass_filter: (cfg.high_pass || cfg.echo_cancellation)
                .then(HighPassFilter::default),
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
                    headroom_db: AGC2_HEADROOM_DB,
                    ..AdaptiveDigital::default()
                }),
                fixed_digital: FixedDigital::default(),
            }),
            ..Default::default()
        };
        let apm = sonora::AudioProcessing::builder()
            .config(config)
            .capture_config(sonora::StreamConfig::new(SAMPLE_RATE, 1))
            .render_config(sonora::StreamConfig::new(SAMPLE_RATE, 1))
            .build();
        Apm {
            inner: Some(apm),
            scratch: [0.0; BLOCK_SAMPLES],
        }
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
        match self.inner.as_mut() {
            Some(apm) => {
                if apm.process_capture_f32(&[x], &mut [out]).is_err() {
                    debug_assert!(false, "sonora rejected a capture block");
                    out.copy_from_slice(x);
                }
            }
            None => out.copy_from_slice(x),
        }
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
