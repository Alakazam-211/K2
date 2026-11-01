//! Engine configuration.

use crate::apm::ProcessingConfig;
use crate::device::NullDevice;
use crate::error::{AudioError, ErrorCode};
#[cfg(feature = "cpal")]
use crate::events::DeviceChoice;
use crate::frame::EncoderCfg;
use crate::jitter::JitterConfig;
use std::time::Duration;

/// Where audio comes from and goes to.
#[derive(Debug)]
#[non_exhaustive]
pub enum Backend {
    /// The system's audio devices (cpal: CoreAudio, WASAPI or ALSA).
    #[cfg(feature = "cpal")]
    System {
        /// The capture device to start with.
        input: DeviceChoice,
        /// The playback device to start with.
        output: DeviceChoice,
    },
    /// No hardware: see [`NullDevice`].
    Null(NullDevice),
}

impl Default for Backend {
    /// The system default devices when built with the `cpal` feature, else
    /// a real-time null device.
    fn default() -> Self {
        #[cfg(feature = "cpal")]
        {
            Backend::System {
                input: DeviceChoice::Default,
                output: DeviceChoice::Default,
            }
        }
        #[cfg(not(feature = "cpal"))]
        {
            Backend::Null(NullDevice::default())
        }
    }
}

impl Backend {
    fn null_device(&self) -> Option<&NullDevice> {
        match self {
            Backend::Null(d) => Some(d),
            #[cfg(feature = "cpal")]
            Backend::System { .. } => None,
        }
    }
}

/// Everything [`crate::AudioEngine::start`] needs.
#[derive(Debug)]
pub struct EngineConfig {
    /// Devices.
    pub backend: Backend,
    /// Capture processing (echo cancellation, noise suppression, gain).
    pub processing: ProcessingConfig,
    /// Initial encoder settings.
    pub encoder: EncoderCfg,
    /// Jitter-buffer settings for every participant.
    pub jitter: JitterConfig,
    /// The first capture timestamp ([`crate::EncodedFrame::ts48k`]). RTP
    /// recommends a random start.
    pub initial_ts48k: u32,
    /// Playback queued ahead of the output device, milliseconds (10..=200).
    /// Lower is less latency; higher survives scheduling hiccups.
    pub playout_buffer_ms: u32,
    /// Capacity of the encoded-frame queue, in frames (20 ms each).
    pub capture_queue_frames: usize,
    /// Capacity of the receive queue, in frames.
    pub recv_queue_frames: usize,
    /// The most participants at once.
    pub max_participants: usize,
    /// Level reading interval, milliseconds (multiple of 10).
    pub levels_interval_ms: u32,
    /// Level at or above which a stream counts as speaking, dBov.
    pub speaking_threshold_dbov: f32,
    /// How often to look for added or removed devices.
    pub device_poll_interval: Duration,
    /// How often the processing thread wakes to move audio. Must be well
    /// under 10 ms.
    pub processing_poll: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            backend: Backend::default(),
            processing: ProcessingConfig::default(),
            encoder: EncoderCfg::default(),
            jitter: JitterConfig::default(),
            initial_ts48k: 0,
            playout_buffer_ms: 30,
            capture_queue_frames: 50,
            recv_queue_frames: 1024,
            max_participants: 32,
            levels_interval_ms: 50,
            speaking_threshold_dbov: -45.0,
            device_poll_interval: Duration::from_secs(1),
            processing_poll: Duration::from_millis(2),
        }
    }
}

impl EngineConfig {
    /// A config for the null device, everything else default.
    pub fn null(device: NullDevice) -> Self {
        EngineConfig {
            backend: Backend::Null(device),
            ..EngineConfig::default()
        }
    }

    pub(crate) fn validate(&self) -> Result<(), AudioError> {
        let bad = |what: &str| Err(AudioError::new(ErrorCode::InvalidConfig, what.to_string()));
        if !(10..=200).contains(&self.playout_buffer_ms) {
            return bad("playout_buffer_ms must be 10..=200");
        }
        if self.capture_queue_frames == 0 || self.recv_queue_frames == 0 {
            return bad("queue sizes must be at least 1");
        }
        if self.max_participants == 0 {
            return bad("max_participants must be at least 1");
        }
        if self.levels_interval_ms < 10 || self.levels_interval_ms % 10 != 0 {
            return bad("levels_interval_ms must be a multiple of 10");
        }
        if self.jitter.min_delay_ms > self.jitter.max_delay_ms {
            return bad("jitter min_delay_ms is above max_delay_ms");
        }
        if self.processing_poll.is_zero() || self.processing_poll >= Duration::from_millis(10) {
            return bad("processing_poll must be between 0 and 10 ms");
        }
        if let Some(d) = self.backend.null_device() {
            if d.sample_rate < 8_000 || d.sample_rate > 384_000 || d.sample_rate % 100 != 0 {
                return bad("null device sample_rate must be 8000..=384000 and a multiple of 100");
            }
            if d.block_period.is_zero() {
                return bad("null device block_period must be above 0");
            }
        }
        Ok(())
    }
}
