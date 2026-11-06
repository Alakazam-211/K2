//! The phone backend: the OS's own voice processing. On iOS that is the
//! VoiceProcessingIO audio unit, on Android AAudio with the
//! voice-communication input preset and usage.
//!
//! The app owns the OS's audio session. This backend never configures or
//! activates it (on iOS, CallKit activates it and the app sets its category);
//! it opens its device when the app says the session is live, closes it when
//! the app says it isn't, and reopens it on a route change.
//!
//! The decisions are plain code over the [`PhoneIo`] trait, so they run
//! with a fake on any OS (see the tests). The OS calls are in `ios` and
//! `android`. Everything here runs on the engine's control thread.

use crate::apm::ProcessingConfig;
use crate::device::{ring_capacity, CaptureIo, PlayIo};
use crate::engine::{is_default, lock, Ctrl, PhoneCmd, Shared};
use crate::error::{AudioError, ErrorCode};
use crate::events::{DeviceChoice, DeviceInfo, DeviceList, DeviceSelection, Event};
use crate::pipeline::Pause;
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(all(feature = "os-voice", target_os = "android"))]
mod android;
#[cfg(all(feature = "os-voice", target_os = "ios"))]
mod ios;
#[cfg(test)]
pub(crate) mod tests;

/// Which voice processing the OS does on the capture path. kessel-audio
/// runs its own (sonora) for whatever the OS doesn't, if
/// [`crate::EngineConfig::processing`] asks for it.
///
/// - **iOS:** the VoiceProcessingIO unit always cancels echo and suppresses
///   noise, so `echo_cancellation` and `noise_suppression` must be `true`.
///   `gain_control: false` turns the unit's gain control off (and lets
///   sonora's run instead).
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
    pub(crate) fn remaining(&self, asked: &ProcessingConfig) -> ProcessingConfig {
        ProcessingConfig {
            echo_cancellation: asked.echo_cancellation && !self.echo_cancellation,
            noise_suppression: if self.noise_suppression {
                None
            } else {
                asked.noise_suppression
            },
            gain_control: asked.gain_control && !self.gain_control,
            high_pass: asked.high_pass && !self.echo_cancellation,
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
    pub(crate) fn validate(&self) -> Result<(), AudioError> {
        #[cfg(target_os = "ios")]
        if !self.os_effects.echo_cancellation || !self.os_effects.noise_suppression {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                "on iOS the voice processing unit always cancels echo and suppresses noise: \
                 os_effects.echo_cancellation and noise_suppression must be true",
            ));
        }
        Ok(())
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
    /// Open capture and playback and start them.
    fn open(&mut self, ends: Ends) -> Result<Opened, AudioError>;
}

/// This OS's phone device.
#[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
pub(crate) fn system_io(cfg: &OsVoiceConfig) -> Box<dyn PhoneIo> {
    #[cfg(target_os = "ios")]
    {
        Box::new(ios::VoiceProcessingIo::new(cfg.os_effects))
    }
    #[cfg(target_os = "android")]
    {
        let _ = cfg;
        Box::new(android::VoiceCommunication)
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
    /// [`PhoneCmd::Session`]).
    pub(crate) fn start(shared: &Arc<Shared>, io: Box<dyn PhoneIo>, session_active: bool) -> Phone {
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
        let mut p = Phone {
            io,
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
        }
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
        let r = self.io.open(ends).and_then(|o| {
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
