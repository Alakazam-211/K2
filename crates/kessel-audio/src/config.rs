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
    /// The phone's own voice processing (echo cancellation, noise
    /// suppression and gain control tuned for the device). iOS: the
    /// VoiceProcessingIO audio unit. Android: AAudio with the
    /// voice-communication input preset and usage (API 28 or later).
    ///
    /// The app owns the OS's audio session; the engine never configures or
    /// activates it. With CallKit, start with
    /// [`crate::AudioEngine::start_when_session_active`] and report
    /// `didActivate` / `didDeactivate` to
    /// [`crate::AudioEngine::session_activated`] /
    /// [`crate::AudioEngine::session_deactivated`], and route changes to
    /// [`crate::AudioEngine::route_changed`]. sonora runs only what
    /// [`crate::OsEffects`] says the OS doesn't do.
    ///
    /// [`crate::PhoneMode::Raw`] (from the start with
    /// [`EngineConfig::with_phone_mode`], or mid-call with
    /// [`crate::AudioEngine::set_phone_mode`]) turns the OS's processing off
    /// for the full microphone signal: sonora cancels echo and controls
    /// gain, and nothing suppresses noise.
    #[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
    OsVoiceProcessing(crate::OsVoiceConfig),
    /// The phone backend's state machine on a fake device (tests).
    #[cfg(test)]
    #[allow(private_interfaces)]
    TestPhone(crate::phone::tests::FakeIo),
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
        // With only the null backend compiled in, the last arm can't match.
        #[allow(unreachable_patterns)]
        match self {
            Backend::Null(d) => Some(d),
            _ => None,
        }
    }

    /// A phone backend (the only kind with a deferred start).
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub(crate) fn is_phone(&self) -> bool {
        self.os_effects().is_some()
    }

    /// What the OS's voice processing covers, for a phone backend.
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub(crate) fn os_effects(&self) -> Option<crate::phone::OsEffects> {
        match self {
            #[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
            Backend::OsVoiceProcessing(c) => Some(c.os_effects),
            #[cfg(test)]
            Backend::TestPhone(f) => Some(f.1),
            _ => None,
        }
    }
}

/// Everything [`crate::AudioEngine::start`] needs.
///
/// `#[non_exhaustive]`: start from [`EngineConfig::default`] (the system
/// devices) or [`EngineConfig::null`] and chain the `with_` methods, or set
/// fields on a `let mut`.
#[derive(Debug)]
#[non_exhaustive]
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
    /// How often to look for added or removed devices (hot-plug). Zero
    /// turns polling off; device errors are still handled.
    pub device_poll_interval: Duration,
    /// How often the processing thread wakes to move audio. Must be well
    /// under 10 ms.
    pub processing_poll: Duration,
    /// How the processing thread asks the OS to schedule it (macOS and iOS;
    /// elsewhere it is reported as [`crate::DspSchedMode::Unsupported`]).
    /// The default, [`crate::DspSchedMode::Workgroup`], is 0.2.4's
    /// behaviour. Read once, as the thread starts; what it got is in
    /// [`crate::Stats::dsp_sched`] and [`crate::Stats::dsp_scheduling`].
    /// [`crate::DspSchedConfig::from_env`] and
    /// [`crate::DspSchedConfig::from_lookup`] read one from the environment
    /// or an app's settings.
    pub dsp_sched: crate::DspSchedConfig,
    /// The phone backend's starting mode: the OS's voice processing (the
    /// default) or raw capture with no noise suppression. Change it during
    /// a call with [`crate::AudioEngine::set_phone_mode`]. Other backends
    /// ignore it.
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub phone_mode: crate::phone::PhoneMode,
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
            dsp_sched: crate::DspSchedConfig::default(),
            #[cfg(any(
                test,
                all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
            ))]
            phone_mode: crate::phone::PhoneMode::Os,
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

    /// Set [`backend`](Self::backend).
    pub fn with_backend(mut self, backend: Backend) -> Self {
        self.backend = backend;
        self
    }

    /// Set [`processing`](Self::processing).
    pub fn with_processing(mut self, processing: ProcessingConfig) -> Self {
        self.processing = processing;
        self
    }

    /// Set the capture stage ([`ProcessingConfig::capture_stage`]), keeping
    /// the rest of [`processing`](Self::processing).
    pub fn with_capture_stage(mut self, stage: Box<dyn crate::CaptureStage>) -> Self {
        self.processing.capture_stage = Some(stage);
        self
    }

    /// Set [`encoder`](Self::encoder).
    pub fn with_encoder(mut self, encoder: EncoderCfg) -> Self {
        self.encoder = encoder;
        self
    }

    /// Set [`jitter`](Self::jitter).
    pub fn with_jitter(mut self, jitter: JitterConfig) -> Self {
        self.jitter = jitter;
        self
    }

    /// Set [`initial_ts48k`](Self::initial_ts48k).
    pub fn with_initial_ts48k(mut self, ts48k: u32) -> Self {
        self.initial_ts48k = ts48k;
        self
    }

    /// Set [`playout_buffer_ms`](Self::playout_buffer_ms).
    pub fn with_playout_buffer_ms(mut self, ms: u32) -> Self {
        self.playout_buffer_ms = ms;
        self
    }

    /// Set [`capture_queue_frames`](Self::capture_queue_frames).
    pub fn with_capture_queue_frames(mut self, frames: usize) -> Self {
        self.capture_queue_frames = frames;
        self
    }

    /// Set [`recv_queue_frames`](Self::recv_queue_frames).
    pub fn with_recv_queue_frames(mut self, frames: usize) -> Self {
        self.recv_queue_frames = frames;
        self
    }

    /// Set [`max_participants`](Self::max_participants).
    pub fn with_max_participants(mut self, n: usize) -> Self {
        self.max_participants = n;
        self
    }

    /// Set [`levels_interval_ms`](Self::levels_interval_ms).
    pub fn with_levels_interval_ms(mut self, ms: u32) -> Self {
        self.levels_interval_ms = ms;
        self
    }

    /// Set [`speaking_threshold_dbov`](Self::speaking_threshold_dbov).
    pub fn with_speaking_threshold_dbov(mut self, dbov: f32) -> Self {
        self.speaking_threshold_dbov = dbov;
        self
    }

    /// Set [`device_poll_interval`](Self::device_poll_interval).
    pub fn with_device_poll_interval(mut self, every: Duration) -> Self {
        self.device_poll_interval = every;
        self
    }

    /// Set [`processing_poll`](Self::processing_poll).
    pub fn with_processing_poll(mut self, every: Duration) -> Self {
        self.processing_poll = every;
        self
    }

    /// Set [`dsp_sched`](Self::dsp_sched).
    pub fn with_dsp_sched(mut self, sched: crate::DspSchedConfig) -> Self {
        self.dsp_sched = sched;
        self
    }

    /// Set [`phone_mode`](Self::phone_mode).
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub fn with_phone_mode(mut self, mode: crate::phone::PhoneMode) -> Self {
        self.phone_mode = mode;
        self
    }

    /// With a phone backend: what the caller asked sonora for, the stage's
    /// latency and what the OS does, kept for mode changes.
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub(crate) fn phone_processing(&self) -> Option<crate::phone::Processing> {
        Some(crate::phone::Processing {
            fx: self.backend.os_effects()?,
            asked: self.processing.settings(),
            stage_latency: self
                .processing
                .capture_stage
                .as_ref()
                .map(|s| s.latency_samples()),
        })
    }

    /// With a phone backend, sonora runs only what the OS doesn't (in
    /// [`crate::phone::PhoneMode::Raw`], echo cancellation and gain control
    /// and no noise suppression). A capture stage stays: it is the caller's,
    /// not one of the OS's effects.
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub(crate) fn with_os_effects(mut self) -> Self {
        if let Some(pp) = self.phone_processing() {
            let left = pp.settings(self.phone_mode);
            let p = &mut self.processing;
            p.echo_cancellation = left.echo_cancellation;
            p.noise_suppression = left.noise_suppression;
            p.gain_control = left.gain_control;
            p.high_pass = left.high_pass;
            p.stage_fallback_noise_suppression = left.stage_fallback_ns;
        }
        self
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
        if self.levels_interval_ms < 10 || !self.levels_interval_ms.is_multiple_of(10) {
            return bad("levels_interval_ms must be a multiple of 10");
        }
        if self.jitter.min_delay_ms > self.jitter.max_delay_ms {
            return bad("jitter min_delay_ms is above max_delay_ms");
        }
        if self.processing_poll.is_zero() || self.processing_poll >= Duration::from_millis(10) {
            return bad("processing_poll must be between 0 and 10 ms");
        }
        self.dsp_sched.validate()?;
        if self.processing.stage_grace > crate::stage::MAX_STAGE_GRACE {
            return bad("processing.stage_grace must be at most 10 s");
        }
        if self.processing.stage_starvation_cutoff < crate::stage::HARD_BUDGET {
            return bad("processing.stage_starvation_cutoff must be at least 9 ms");
        }
        #[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
        if let Backend::OsVoiceProcessing(c) = &self.backend {
            c.validate(self.phone_mode)?;
        }
        if let Some(d) = self.backend.null_device() {
            if d.sample_rate < 8_000
                || d.sample_rate > 384_000
                || !d.sample_rate.is_multiple_of(100)
            {
                return bad("null device sample_rate must be 8000..=384000 and a multiple of 100");
            }
            if d.block_period.is_zero() {
                return bad("null device block_period must be above 0");
            }
        }
        Ok(())
    }
}
