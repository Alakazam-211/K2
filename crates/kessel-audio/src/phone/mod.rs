//! The phone backend: the OS's own voice processing. On iOS that is the
//! VoiceProcessingIO audio unit, on Android AAudio with the
//! voice-communication input preset and usage. In [`PhoneMode::Raw`] the OS
//! processes nothing (iOS: the plain RemoteIO unit; Android: the
//! unprocessed or voice-recognition preset) and sonora cancels echo and
//! controls gain, with no noise suppression.
//!
//! The app owns the OS's audio session. This backend never configures or
//! activates it (on iOS, CallKit activates it and the app sets its category);
//! it opens its device when the app says the session is live, closes it when
//! the app says it isn't, and reopens it on a route change.
//!
//! The decisions are plain code over the [`PhoneIo`] trait, so they run
//! with a fake on any OS (see the tests). The OS calls are in `ios` and
//! `android`. Everything here runs on the engine's control thread.

use crate::apm::{ApmSettings, Chain};
use crate::device::{ring_capacity, CaptureIo, PlayIo};
use crate::engine::{is_default, lock, Ctrl, PhoneCmd, Shared};
use crate::error::{AudioError, ErrorCode};
use crate::events::{DeviceChoice, DeviceInfo, DeviceList, DeviceSelection, Event};
use crate::pipeline::Pause;
use crossbeam_queue::ArrayQueue;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(all(feature = "os-voice", target_os = "android"))]
mod android;
#[cfg(all(feature = "os-voice", target_os = "ios"))]
mod ios;
#[cfg(test)]
pub(crate) mod tests;

/// Which voice processing the OS does on the capture path in
/// [`PhoneMode::Os`]. kessel-audio runs its own (sonora) for whatever the OS
/// doesn't, if [`crate::EngineConfig::processing`] asks for it.
/// [`PhoneMode::Raw`] ignores these: there the OS does none of it.
///
/// - **iOS:** the VoiceProcessingIO unit always cancels echo and suppresses
///   noise together, so either `echo_cancellation` and `noise_suppression`
///   are both `true` (that unit; `gain_control: false` turns its gain
///   control off and lets sonora's run instead), or all three are `false`:
///   the plain RemoteIO unit, no OS processing, and sonora runs everything
///   [`crate::EngineConfig::processing`] asks for (since 0.2.1).
/// - **Android:** the voice-communication preset applies whatever the device
///   provides. The app knows which (`AcousticEchoCanceler.isAvailable()`,
///   `NoiseSuppressor.isAvailable()`, `AutomaticGainControl.isAvailable()`)
///   and sets these to match; sonora fills the gaps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsEffects {
    /// The OS cancels echo. Its echo reference is its own, so sonora's echo
    /// canceller stays off.
    pub echo_cancellation: bool,
    /// The OS suppresses noise.
    pub noise_suppression: bool,
    /// The OS controls the microphone gain.
    pub gain_control: bool,
}

impl Default for OsEffects {
    /// All three: the OS does all the voice processing.
    fn default() -> Self {
        OsEffects {
            echo_cancellation: true,
            noise_suppression: true,
            gain_control: true,
        }
    }
}

impl OsEffects {
    /// The processing left for sonora: what the caller asked for, less what
    /// the OS does. The high-pass filter goes with echo cancellation (the
    /// OS's filters its own input).
    pub(crate) fn remaining(&self, asked: ApmSettings) -> ApmSettings {
        ApmSettings {
            echo_cancellation: asked.echo_cancellation && !self.echo_cancellation,
            noise_suppression: if self.noise_suppression {
                None
            } else {
                asked.noise_suppression
            },
            gain_control: asked.gain_control && !self.gain_control,
            high_pass: asked.high_pass && !self.echo_cancellation,
            // The OS's suppressor already runs before any stage.
            stage_fallback_ns: if self.noise_suppression {
                None
            } else {
                asked.stage_fallback_ns
            },
        }
    }
}

/// Settings for `Backend::OsVoiceProcessing`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OsVoiceConfig {
    /// What the OS's voice processing covers.
    pub os_effects: OsEffects,
}

impl OsVoiceConfig {
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    pub(crate) fn validate(&self, mode: PhoneMode) -> Result<(), AudioError> {
        #[cfg(target_os = "ios")]
        if !ios_effects_ok(self.os_effects, mode) {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                "on iOS the voice processing unit always cancels echo and suppresses noise: \
                 os_effects.echo_cancellation and noise_suppression must both be true, \
                 or all three false (no OS processing)",
            ));
        }
        #[cfg(not(target_os = "ios"))]
        let _ = mode;
        Ok(())
    }
}

/// iOS takes the voice processing unit's effects (echo cancellation and
/// noise suppression, gain control either way) or none at all. Raw mode
/// ignores them.
#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) fn ios_effects_ok(fx: OsEffects, mode: PhoneMode) -> bool {
    let none = !fx.echo_cancellation && !fx.noise_suppression && !fx.gain_control;
    mode.is_raw() || none || (fx.echo_cancellation && fx.noise_suppression)
}

/// How the phone backend captures: through the OS's voice processing (the
/// default, as before 0.2.1) or raw.
///
/// Set it at the start with [`crate::EngineConfig::with_phone_mode`] and
/// change it mid-call with [`crate::AudioEngine::set_phone_mode`]. A change
/// while the device is open stops and reopens it in the new mode (a short
/// gap, reported in [`crate::Event::PhoneMode`] and [`crate::Stats`]); the
/// call, its participants, the encoder and the capture stage carry on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum PhoneMode {
    /// The OS's voice processing, as [`OsVoiceConfig::os_effects`] says.
    /// sonora runs only what the OS doesn't.
    #[default]
    Os,
    /// No OS voice processing: the full microphone signal, background noise
    /// and all. sonora cancels echo (AEC3 against the playback mix, as on
    /// desktop, with its high-pass filter) and controls gain (AGC2), and
    /// suppresses no noise: neither
    /// [`ProcessingConfig::noise_suppression`](crate::ProcessingConfig::noise_suppression)
    /// nor the stage's step-down floor runs. This holds whatever
    /// [`crate::EngineConfig::processing`] and [`OsVoiceConfig::os_effects`]
    /// say; a capture stage, if set, still runs (it is the caller's).
    ///
    /// - **iOS:** the plain RemoteIO unit instead of VoiceProcessingIO.
    /// - **Android:** the input uses AAudio's unprocessed preset when
    ///   [`RawCapture::unprocessed_supported`] is set, else the
    ///   voice-recognition preset, and has no audio session, so no
    ///   `NoiseSuppressor` (or other `AudioEffect`) can attach to it.
    ///   Playback keeps the voice-communication usage.
    Raw(RawCapture),
}

impl PhoneMode {
    /// [`PhoneMode::Raw`] with [`RawCapture::default`].
    pub fn raw() -> PhoneMode {
        PhoneMode::Raw(RawCapture::default())
    }

    /// True for [`PhoneMode::Raw`].
    pub fn is_raw(&self) -> bool {
        matches!(self, PhoneMode::Raw(_))
    }
}

/// Settings for [`PhoneMode::Raw`].
///
/// `#[non_exhaustive]`: start from [`RawCapture::default`] and use the
/// `with_` methods.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RawCapture {
    /// **Android:** the device supports the unprocessed audio source: what
    /// `AudioManager.getProperty(AudioManager.PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED)`
    /// says (`"true"`). kessel-audio can't ask (it has no Java side), so the
    /// app does. `false` (the default) uses the voice-recognition preset,
    /// which Android's compatibility rules keep free of noise reduction.
    /// Ignored on iOS.
    pub unprocessed_supported: bool,
}

impl RawCapture {
    /// Set [`unprocessed_supported`](Self::unprocessed_supported).
    pub fn with_unprocessed_supported(mut self, supported: bool) -> Self {
        self.unprocessed_supported = supported;
        self
    }
}

/// A phone mode change ([`crate::Event::PhoneMode`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PhoneModeChange {
    /// The mode now in use.
    pub mode: PhoneMode,
    /// How long the device was stopped for the change: from stopping it in
    /// the old mode to it running in the new one. `None` when no device was
    /// open (the session inactive, or the call suspended): the new mode
    /// applies when it opens. Capture frames after the gap carry it in
    /// their `ts48k`, as after [`crate::AudioEngine::suspend`].
    pub gap: Option<Duration>,
}

/// What sonora runs in [`PhoneMode::Raw`]: echo cancellation (with the
/// high-pass filter) and gain control, no noise suppression.
pub(crate) const RAW_PROCESSING: ApmSettings = ApmSettings {
    echo_cancellation: true,
    noise_suppression: None,
    gain_control: true,
    high_pass: true,
    stage_fallback_ns: None,
};

/// Which iOS audio unit opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IosUnit {
    /// VoiceProcessingIO: the OS's echo cancellation and noise suppression.
    VoiceProcessing,
    /// The plain RemoteIO unit: no OS processing.
    RemoteIo,
}

#[cfg_attr(not(any(test, target_os = "ios")), allow(dead_code))]
pub(crate) fn ios_unit(fx: OsEffects, mode: PhoneMode) -> IosUnit {
    if mode.is_raw() || !fx.echo_cancellation {
        IosUnit::RemoteIo
    } else {
        IosUnit::VoiceProcessing
    }
}

/// Android's capture setup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AndroidInput {
    pub(crate) preset: AndroidPreset,
    /// Open the input with no audio session (`AAUDIO_SESSION_ID_NONE`), so
    /// no `AudioEffect` can attach.
    pub(crate) no_session: bool,
}

/// AAudio's input presets that the backend uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AndroidPreset {
    VoiceCommunication,
    Unprocessed,
    VoiceRecognition,
}

#[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
pub(crate) fn android_input(mode: PhoneMode) -> AndroidInput {
    match mode {
        PhoneMode::Os => AndroidInput {
            preset: AndroidPreset::VoiceCommunication,
            no_session: false,
        },
        PhoneMode::Raw(r) => AndroidInput {
            preset: if r.unprocessed_supported {
                AndroidPreset::Unprocessed
            } else {
                AndroidPreset::VoiceRecognition
            },
            no_session: true,
        },
    }
}

/// What sonora runs in each mode, and how to build it: the caller's
/// processing settings and stage latency, kept for mode changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Processing {
    pub(crate) fx: OsEffects,
    /// What [`crate::EngineConfig::processing`] asked for.
    pub(crate) asked: ApmSettings,
    /// The capture stage's latency, samples (`None` without one).
    pub(crate) stage_latency: Option<usize>,
}

impl Processing {
    pub(crate) fn settings(&self, mode: PhoneMode) -> ApmSettings {
        match mode {
            PhoneMode::Os => self.fx.remaining(self.asked),
            PhoneMode::Raw(_) => RAW_PROCESSING,
        }
    }

    /// Build the sonora chain for `mode` (allocates: control thread only).
    pub(crate) fn chain(&self, mode: PhoneMode) -> Box<Chain> {
        Chain::new(self.settings(mode), self.stage_latency)
    }
}

/// The highest device rate the backend takes (the rings are sized for it
/// before the OS says what it opened).
pub(crate) const MAX_RATE: u32 = 96_000;
/// How long to wait before trying a failed open again.
pub(crate) const RETRY: Duration = Duration::from_secs(1);

pub(crate) const INPUT_ID: &str = "os-voice:input";
pub(crate) const OUTPUT_ID: &str = "os-voice:output";

/// Where a device thread reports that its stream died (Android: the stream
/// was disconnected by a route change). Lock-free; safe in a callback.
#[derive(Clone)]
pub(crate) struct Faults {
    q: Arc<ArrayQueue<u64>>,
    generation: u64,
}

impl Faults {
    #[cfg_attr(
        not(all(feature = "os-voice", target_os = "android")),
        allow(dead_code)
    )]
    pub(crate) fn raise(&self) {
        let _ = self.q.push(self.generation);
    }
}

/// What the device callbacks get: the two sample rings, the counters (in
/// `shared.io`) and the fault line.
pub(crate) struct Ends {
    pub(crate) capture: rtrb::Producer<f32>,
    pub(crate) playback: rtrb::Consumer<f32>,
    pub(crate) shared: Arc<Shared>,
    /// iOS reports route changes through the app instead.
    #[cfg_attr(not(any(test, target_os = "android")), allow(dead_code))]
    pub(crate) faults: Faults,
}

/// An open device.
pub(crate) struct Opened {
    /// Keeps the OS stream running; dropping it stops and closes it, after
    /// which no callback touches the rings.
    pub(crate) stream: Box<dyn std::any::Any>,
    /// The capture rate, or `None` when only playback opened.
    pub(crate) capture_rate: Option<u32>,
    pub(crate) playback_rate: u32,
    /// For [`crate::DevicesInUse`].
    pub(crate) name: &'static str,
    /// Problems that didn't stop the device (no microphone access, a
    /// setting the OS refused). Raised as [`Event::Error`].
    pub(crate) warnings: Vec<AudioError>,
}

/// The OS's side of the phone backend.
pub(crate) trait PhoneIo: Send {
    /// The OS's current device rate, if it can say. A route change reopens
    /// the device unless this matches the rate it has open.
    fn current_rate(&self) -> Option<u32>;
    /// Open capture and playback in `mode` and start them.
    fn open(&mut self, ends: Ends, mode: PhoneMode) -> Result<Opened, AudioError>;
}

/// This OS's phone device.
#[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
pub(crate) fn system_io(cfg: &OsVoiceConfig) -> Box<dyn PhoneIo> {
    #[cfg(target_os = "ios")]
    {
        Box::new(ios::RemoteUnit::new(cfg.os_effects))
    }
    #[cfg(target_os = "android")]
    {
        let _ = cfg;
        Box::new(android::AAudioIo)
    }
}

struct Live {
    // Held to keep the device running.
    _stream: Box<dyn std::any::Any>,
    generation: u64,
    capture_rate: Option<u32>,
    playback_rate: u32,
}

#[derive(Clone, Copy)]
struct Failed {
    code: ErrorCode,
    retry_at: Instant,
}

/// The phone backend's state machine. The device is open exactly while the
/// session is active and the call isn't suspended.
pub(crate) struct Phone {
    io: Box<dyn PhoneIo>,
    mode: PhoneMode,
    processing: Processing,
    session_active: bool,
    suspended: bool,
    open: Option<Live>,
    generation: u64,
    faults: Arc<ArrayQueue<u64>>,
    failed: Option<Failed>,
}

fn info(id: &str, what: &str, name: &str, rate: Option<u32>) -> DeviceInfo {
    DeviceInfo {
        id: id.into(),
        name: match rate {
            Some(r) => format!("{name} {what} ({r} Hz)"),
            None => format!("{name} {what}"),
        },
        is_default: true,
    }
}

const LIST_NAME: &str = "OS voice processing";

impl Phone {
    /// `session_active`: open the device now (`false` waits for
    /// [`PhoneCmd::Session`]). The processing thread already runs the chain
    /// for `mode` (the engine built it from the same settings).
    pub(crate) fn start(
        shared: &Arc<Shared>,
        io: Box<dyn PhoneIo>,
        session_active: bool,
        processing: Processing,
        mode: PhoneMode,
    ) -> Phone {
        let list = DeviceList {
            inputs: vec![info(INPUT_ID, "input", LIST_NAME, None)],
            outputs: vec![info(OUTPUT_ID, "output", LIST_NAME, None)],
        };
        *lock(&shared.devices) = list.clone();
        shared.emit(Event::Devices(list));
        // No device yet: the capture path waits, and its first frame after a
        // later pause carries the gap.
        shared.send_ctrl(Ctrl::Pause {
            why: Pause::Device,
            on: true,
        });
        *lock(&shared.phone_mode) = mode;
        let mut p = Phone {
            io,
            mode,
            processing,
            session_active,
            suspended: false,
            open: None,
            generation: 0,
            faults: Arc::new(ArrayQueue::new(16)),
            failed: None,
        };
        p.reconcile(shared, false);
        p
    }

    pub(crate) fn command(&mut self, shared: &Arc<Shared>, cmd: PhoneCmd) {
        // Any command is news from the app: a failed open is tried again now.
        self.failed = None;
        match cmd {
            PhoneCmd::Session(on) => {
                self.session_active = on;
                self.reconcile(shared, false);
            }
            PhoneCmd::Suspend(on) => {
                self.suspended = on;
                self.reconcile(shared, false);
            }
            PhoneCmd::RouteChanged => {
                let same = match (&self.open, self.io.current_rate()) {
                    (Some(l), Some(r)) => l.playback_rate == r && l.capture_rate == Some(r),
                    _ => false,
                };
                self.reconcile(shared, !same);
            }
            PhoneCmd::Mode(m) => self.set_mode(shared, m),
        }
    }

    /// Change mode. With the device open: stop it, swap sonora's chain,
    /// open it in the new mode. Nothing else in the engine stops. If the
    /// new mode won't open, go back to the old one (which was open a moment
    /// ago) rather than leave the call without audio.
    fn set_mode(&mut self, shared: &Arc<Shared>, mode: PhoneMode) {
        if mode == self.mode {
            return;
        }
        // Built here, not on the processing thread: it allocates.
        let chain = self.processing.chain(mode);
        let prev = self.mode;
        let was_open = self.open.is_some();
        let t0 = Instant::now();
        self.close(shared);
        self.mode = mode;
        // Queued after the close and before the open's messages, so the
        // processing thread swaps chains while the capture path is paused:
        // no block of either device meets the other mode's chain.
        shared.send_ctrl(Ctrl::Processing(chain));
        self.reconcile(shared, false);
        if was_open && self.open.is_none() {
            self.mode = prev;
            shared.send_ctrl(Ctrl::Processing(self.processing.chain(prev)));
            self.failed = None;
            self.reconcile(shared, false);
            return;
        }
        let gap = was_open.then(|| t0.elapsed());
        *lock(&shared.phone_mode) = mode;
        shared.mode_switches.fetch_add(1, Ordering::Relaxed);
        if let Some(g) = gap {
            let us = u64::try_from(g.as_micros()).unwrap_or(u64::MAX - 1);
            shared.last_mode_gap_us.store(us, Ordering::Relaxed);
        }
        shared.emit(Event::PhoneMode(PhoneModeChange { mode, gap }));
    }

    pub(crate) fn tick(&mut self, shared: &Arc<Shared>) {
        let current = self.open.as_ref().map(|l| l.generation);
        let mut died = false;
        while let Some(g) = self.faults.pop() {
            died |= Some(g) == current;
        }
        if died {
            // The OS dropped the stream (on Android, a route change does
            // this): reopen on the new route, as a route change would.
            self.reconcile(shared, true);
            return;
        }
        if let Some(f) = self.failed {
            if Instant::now() >= f.retry_at {
                self.reconcile(shared, false);
            }
        }
    }

    fn want(&self) -> bool {
        self.session_active && !self.suspended
    }

    fn reconcile(&mut self, shared: &Arc<Shared>, reopen: bool) {
        if !self.want() {
            self.close(shared);
            self.failed = None;
            return;
        }
        if self.open.is_some() && !reopen {
            return;
        }
        self.close(shared);
        self.open(shared);
    }

    fn close(&mut self, shared: &Arc<Shared>) {
        let Some(live) = self.open.take() else {
            return;
        };
        shared.send_ctrl(Ctrl::Pause {
            why: Pause::Device,
            on: true,
        });
        // Stop the OS stream before its rings go.
        drop(live);
        shared.send_ctrl(Ctrl::CaptureIo(None));
        shared.send_ctrl(Ctrl::PlayIo(None));
        shared.set_in_use(|u| {
            u.input = None;
            u.output = None;
        });
    }

    fn open(&mut self, shared: &Arc<Shared>) {
        self.generation += 1;
        let cap = ring_capacity(MAX_RATE, shared.playout_buffer_ms);
        let (cap_tx, cap_rx) = rtrb::RingBuffer::<f32>::new(cap);
        let (play_tx, play_rx) = rtrb::RingBuffer::<f32>::new(cap);
        let ends = Ends {
            capture: cap_tx,
            playback: play_rx,
            shared: shared.clone(),
            faults: Faults {
                q: self.faults.clone(),
                generation: self.generation,
            },
        };
        let playout_ms = shared.playout_buffer_ms;
        let r = self.io.open(ends, self.mode).and_then(|o| {
            let rate_ok = |r: u32| (8_000..=MAX_RATE).contains(&r);
            if !rate_ok(o.playback_rate) || !o.capture_rate.is_none_or(rate_ok) {
                return Err(AudioError::new(
                    ErrorCode::UnsupportedConfig,
                    format!(
                        "device rate {:?} / {} Hz is outside 8000..={MAX_RATE}",
                        o.capture_rate, o.playback_rate
                    ),
                ));
            }
            let capture = match o.capture_rate {
                Some(r) => Some(CaptureIo::new(cap_rx, r)?),
                None => None,
            };
            let playback = PlayIo::new(play_tx, o.playback_rate, playout_ms)?;
            Ok((o, capture, playback))
        });
        match r {
            Ok((o, capture, playback)) => {
                let has_input = capture.is_some();
                if let Some(c) = capture {
                    shared.send_ctrl(Ctrl::CaptureIo(Some(Box::new(c))));
                }
                shared.send_ctrl(Ctrl::PlayIo(Some(Box::new(playback))));
                // Without a microphone the capture path stays paused: no
                // frames, and the gap shows when one opens.
                if has_input {
                    shared.send_ctrl(Ctrl::Pause {
                        why: Pause::Device,
                        on: false,
                    });
                }
                for w in o.warnings {
                    shared.emit(Event::Error(w));
                }
                let (name, cr, pr) = (o.name, o.capture_rate, o.playback_rate);
                shared.set_in_use(|u| {
                    u.input = cr.map(|r| info(INPUT_ID, "input", name, Some(r)));
                    u.output = Some(info(OUTPUT_ID, "output", name, Some(pr)));
                });
                self.open = Some(Live {
                    _stream: o.stream,
                    generation: self.generation,
                    capture_rate: cr,
                    playback_rate: pr,
                });
                self.failed = None;
            }
            Err(e) => {
                // Raised once per kind of failure, not on every retry.
                if self.failed.map(|f| f.code) != Some(e.code) {
                    shared.emit(Event::Error(e.clone()));
                }
                self.failed = Some(Failed {
                    code: e.code,
                    retry_at: Instant::now() + RETRY,
                });
            }
        }
    }
}

/// The OS picks the route (the app's route picker); the backend has one
/// device each way.
pub(crate) fn check_selection(sel: &DeviceSelection) -> Result<(), AudioError> {
    let ok = |c: &Option<DeviceChoice>, id: &str| {
        is_default(c) || matches!(c, Some(DeviceChoice::Id(x)) if x == id)
    };
    if ok(&sel.input, INPUT_ID) && ok(&sel.output, OUTPUT_ID) {
        Ok(())
    } else {
        Err(AudioError::new(
            ErrorCode::NoDevice,
            "the OS chooses the route with this backend: use the system's route picker",
        ))
    }
}
