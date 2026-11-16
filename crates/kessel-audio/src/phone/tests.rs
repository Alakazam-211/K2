//! The phone backend's state machine on a fake OS: deferred start, session
//! activate and deactivate, suspend and resume, route changes, faults and
//! retries, and the mode switch between the OS's voice processing and raw
//! capture. The state machine tests drive it directly; the engine tests run
//! it on the engine's real threads with a fake device that plays a clock.
//!
//! Everything here is a fake: the OS calls themselves (the audio units on
//! iOS, AAudio on Android) are only type-checked. What the fakes prove is
//! the state machine, which path each mode asks the OS for, and what sonora
//! runs.

use super::{
    android_input, check_selection, ios_effects_ok, ios_unit, AndroidPreset, Ends, Faults, IosUnit,
    Opened, OsEffects, Phone, PhoneIo, PhoneMode, PhoneModeChange, Processing, RawCapture,
    RAW_PROCESSING,
};
use crate::apm::{ApmSettings, ProcessingConfig};
use crate::config::{Backend, EngineConfig};
use crate::device::NullDevice;
use crate::engine::{lock, AudioEngine, Ctrl, PhoneCmd, Shared};
use crate::error::{AudioError, ErrorCode};
use crate::events::{DeviceChoice, DeviceSelection, Event};
use crate::frame::EncoderCfg;
use crate::pipeline::Pause;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// `Backend::TestPhone`'s payload: the fake device and what the "OS" does.
pub(crate) struct FakeIo(pub(crate) Box<dyn PhoneIo>, pub(crate) OsEffects);

impl std::fmt::Debug for FakeIo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FakeIo")
            .field(&self.1)
            .finish_non_exhaustive()
    }
}

struct State {
    rate: u32,
    mic: bool,
    fail: Option<ErrorCode>,
    fail_raw: Option<ErrorCode>,
    /// Drive the rings from a thread at `rate`, in real time.
    clock: bool,
    log: Vec<String>,
    faults: Option<Faults>,
}

/// The scripted OS. Clones share it.
#[derive(Clone)]
pub(crate) struct FakeOs(Arc<Mutex<State>>);

impl FakeOs {
    pub(crate) fn new(rate: u32) -> FakeOs {
        FakeOs(Arc::new(Mutex::new(State {
            rate,
            mic: true,
            fail: None,
            fail_raw: None,
            clock: false,
            log: Vec::new(),
            faults: None,
        })))
    }
    fn clocked(self) -> FakeOs {
        lock(&self.0).clock = true;
        self
    }
    fn set_rate(&self, r: u32) {
        lock(&self.0).rate = r;
    }
    fn set_mic(&self, on: bool) {
        lock(&self.0).mic = on;
    }
    fn fail(&self, code: Option<ErrorCode>) {
        lock(&self.0).fail = code;
    }
    /// Only the raw path fails to open.
    fn fail_raw(&self, code: Option<ErrorCode>) {
        lock(&self.0).fail_raw = code;
    }
    /// What happened since the last call.
    fn log(&self) -> Vec<String> {
        std::mem::take(&mut lock(&self.0).log)
    }
    /// The open stream's device thread reports that it died.
    fn raise_fault(&self) {
        lock(&self.0)
            .faults
            .as_ref()
            .expect("a stream was opened")
            .raise();
    }
    fn io(&self) -> Box<dyn PhoneIo> {
        Box::new(FakeDev(self.clone()))
    }
}

struct FakeDev(FakeOs);

/// The open fake stream; dropping it stops its clock and logs the close.
struct Stream {
    os: FakeOs,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    _ends: Option<(rtrb::Producer<f32>, rtrb::Consumer<f32>)>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            t.join().expect("fake clock thread");
        }
        lock(&self.os.0).log.push("close".into());
    }
}

impl PhoneIo for FakeDev {
    fn current_rate(&self) -> Option<u32> {
        Some(lock(&self.0 .0).rate)
    }

    fn open(&mut self, ends: Ends, mode: PhoneMode) -> Result<Opened, AudioError> {
        let mut st = lock(&self.0 .0);
        let rate = st.rate;
        // The log says which path opened: "open 48000" is the OS's voice
        // processing (as before 0.2.1), "open raw 48000" the raw one.
        let what = match mode {
            PhoneMode::Os => format!("open {rate}"),
            PhoneMode::Raw(r) if r.unprocessed_supported => format!("open raw unprocessed {rate}"),
            PhoneMode::Raw(_) => format!("open raw {rate}"),
        };
        let fail = if mode.is_raw() {
            st.fail.or(st.fail_raw)
        } else {
            st.fail
        };
        if let Some(code) = fail {
            st.log.push(format!("{what} failed"));
            return Err(AudioError::new(code, "scripted failure"));
        }
        st.log.push(what);
        st.faults = Some(ends.faults.clone());
        let mic = st.mic;
        let clock = st.clock;
        drop(st);
        let stop = Arc::new(AtomicBool::new(false));
        let (thread, kept) = if clock {
            let Ends {
                mut capture,
                mut playback,
                shared,
                ..
            } = ends;
            let stop = stop.clone();
            let t = std::thread::spawn(move || {
                // A 300 Hz tone at `rate`, 10 ms at a time, in real time.
                let block = (rate / 100) as usize;
                let mut n = 0u64;
                let mut out = vec![0.0f32; block];
                let mut inb = vec![0.0f32; block];
                let mut next = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    for v in inb.iter_mut() {
                        *v = 0.3
                            * (2.0 * std::f32::consts::PI * 300.0 * n as f32 / rate as f32).sin();
                        n += 1;
                    }
                    if mic {
                        crate::device::push_capture(&mut capture, &inb, 1, |v| v, &shared.io);
                    }
                    crate::device::pop_playback(&mut playback, &mut out, 1, |v| v, &shared.io);
                    next += Duration::from_millis(10);
                    if let Some(d) = next.checked_duration_since(Instant::now()) {
                        std::thread::sleep(d);
                    }
                }
            });
            (Some(t), None)
        } else {
            (None, Some((ends.capture, ends.playback)))
        };
        Ok(Opened {
            stream: Box::new(Stream {
                os: self.0.clone(),
                stop,
                thread,
                _ends: kept,
            }),
            capture_rate: mic.then_some(rate),
            playback_rate: rate,
            name: "Fake",
            warnings: if mic {
                Vec::new()
            } else {
                vec![AudioError::new(ErrorCode::NoPermission, "scripted refusal")]
            },
        })
    }
}

// ---- The state machine, driven directly ----

fn shared() -> Arc<Shared> {
    Shared::new(&EngineConfig::null(NullDevice::default()))
}

/// The control messages sent to the processing thread, as words.
fn ctrl(shared: &Shared) -> Vec<String> {
    let mut out = Vec::new();
    while let Some(c) = shared.ctrl.pop() {
        out.push(match c {
            Ctrl::CaptureIo(Some(_)) => "capture".into(),
            Ctrl::CaptureIo(None) => "capture off".into(),
            Ctrl::PlayIo(Some(_)) => "play".into(),
            Ctrl::PlayIo(None) => "play off".into(),
            Ctrl::Pause { why, on } => {
                assert_eq!(why, Pause::Device, "the backend only sends device pauses");
                (if on { "pause" } else { "unpause" }).into()
            }
            Ctrl::Processing(c) => format!("processing {}", describe(c.settings())),
            _ => panic!("unexpected control message"),
        });
    }
    out
}

/// What sonora runs, as words: "hpf+aec+agc", "none", ...
fn describe(s: ApmSettings) -> String {
    let mut on = Vec::new();
    if s.high_pass || s.echo_cancellation {
        on.push("hpf".to_string());
    }
    if s.echo_cancellation {
        on.push("aec".into());
    }
    if let Some(n) = s.noise_suppression {
        on.push(format!("ns {n:?}"));
    }
    if s.gain_control {
        on.push("agc".into());
    }
    if let Some(n) = s.stage_fallback_ns {
        on.push(format!("floor {n:?}"));
    }
    if on.is_empty() {
        "none".into()
    } else {
        on.join("+")
    }
}

/// sonora in raw mode.
const RAW: &str = "processing hpf+aec+agc";
/// sonora in the OS's mode with the OS doing everything: nothing.
const OS: &str = "processing none";

fn events(shared: &Shared) -> Vec<Event> {
    std::iter::from_fn(|| shared.events.pop()).collect()
}

fn errors(shared: &Shared) -> Vec<ErrorCode> {
    events(shared)
        .into_iter()
        .filter_map(|e| match e {
            Event::Error(e) => Some(e.code),
            _ => None,
        })
        .collect()
}

/// Errors and mode changes, in order (devices and levels left out).
fn notices(shared: &Shared) -> Vec<String> {
    events(shared)
        .into_iter()
        .filter_map(|e| match e {
            Event::Error(e) => Some(format!("error {:?}", e.code)),
            Event::PhoneMode(PhoneModeChange { mode, gap }) => Some(format!(
                "mode {} {}",
                if mode.is_raw() { "raw" } else { "os" },
                if gap.is_some() { "restarted" } else { "closed" }
            )),
            _ => None,
        })
        .collect()
}

/// The default caller (everything on, no stage) with the OS doing it all,
/// as Sew starts.
fn procs() -> Processing {
    Processing {
        fx: OsEffects::default(),
        asked: ProcessingConfig::default().settings(),
        stage_latency: None,
    }
}

fn in_use(shared: &Shared) -> (Option<String>, Option<String>) {
    let u = lock(&shared.in_use).clone();
    (u.input.map(|d| d.name), u.output.map(|d| d.name))
}

const OPENED: [&str; 3] = ["capture", "play", "unpause"];
const CLOSED: [&str; 3] = ["pause", "capture off", "play off"];

fn rig(session_active: bool) -> (Arc<Shared>, FakeOs, Phone) {
    rig_in(session_active, PhoneMode::Os)
}

fn rig_in(session_active: bool, mode: PhoneMode) -> (Arc<Shared>, FakeOs, Phone) {
    let s = shared();
    let os = FakeOs::new(48_000);
    let p = start(&s, &os, session_active, mode);
    (s, os, p)
}

fn start(s: &Arc<Shared>, os: &FakeOs, session_active: bool, mode: PhoneMode) -> Phone {
    Phone::start(s, os.io(), session_active, procs(), mode)
}

#[test]
fn a_deferred_start_opens_only_when_the_session_is_active() {
    let (s, os, mut p) = rig(false);
    assert_eq!(
        os.log(),
        Vec::<String>::new(),
        "nothing opens before the session"
    );
    assert_eq!(ctrl(&s), ["pause"], "the capture path waits for a device");
    assert_eq!(in_use(&s), (None, None));
    p.tick(&s);
    assert_eq!(os.log(), Vec::<String>::new());

    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), ["open 48000"]);
    assert_eq!(ctrl(&s), OPENED);
    assert_eq!(
        in_use(&s),
        (
            Some("Fake input (48000 Hz)".into()),
            Some("Fake output (48000 Hz)".into())
        )
    );
    assert_eq!(errors(&s), []);
}

#[test]
fn an_immediate_start_opens_at_once() {
    let (s, os, _p) = rig(true);
    assert_eq!(os.log(), ["open 48000"]);
    let mut want = vec!["pause"];
    want.extend(OPENED);
    assert_eq!(ctrl(&s), want);
}

#[test]
fn deactivating_closes_and_activating_reopens() {
    let (s, os, mut p) = rig(true);
    os.log();
    ctrl(&s);
    p.command(&s, PhoneCmd::Session(false));
    assert_eq!(os.log(), ["close"]);
    assert_eq!(ctrl(&s), CLOSED);
    assert_eq!(in_use(&s), (None, None));
    p.command(&s, PhoneCmd::Session(false));
    assert_eq!(os.log(), Vec::<String>::new(), "deactivating twice is once");
    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), ["open 48000"]);
    assert_eq!(ctrl(&s), OPENED);
    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), Vec::<String>::new(), "activating twice is once");
}

#[test]
fn suspend_closes_and_resume_reopens_only_with_the_session_active() {
    let (s, os, mut p) = rig(true);
    os.log();
    ctrl(&s);
    p.command(&s, PhoneCmd::Suspend(true));
    assert_eq!(os.log(), ["close"]);
    assert_eq!(ctrl(&s), CLOSED);
    // A cellular call: CallKit also deactivates the session around it.
    p.command(&s, PhoneCmd::Session(false));
    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), Vec::<String>::new(), "still suspended");
    p.command(&s, PhoneCmd::Suspend(false));
    assert_eq!(os.log(), ["open 48000"]);
    assert_eq!(ctrl(&s), OPENED);

    // Resumed before the session comes back: opens with the session.
    p.command(&s, PhoneCmd::Suspend(true));
    p.command(&s, PhoneCmd::Session(false));
    p.command(&s, PhoneCmd::Suspend(false));
    assert_eq!(os.log(), ["close"]);
    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), ["open 48000"]);
}

#[test]
fn a_route_change_reopens_at_the_new_rate_mid_call() {
    let (s, os, mut p) = rig(true);
    os.log();
    ctrl(&s);
    // Same rate (headphones in): nothing to do.
    p.command(&s, PhoneCmd::RouteChanged);
    assert_eq!(os.log(), Vec::<String>::new());
    assert_eq!(ctrl(&s), Vec::<String>::new());
    // Bluetooth hands-free: 16 kHz.
    os.set_rate(16_000);
    p.command(&s, PhoneCmd::RouteChanged);
    assert_eq!(os.log(), ["close", "open 16000"]);
    let mut want = CLOSED.to_vec();
    want.extend(OPENED);
    assert_eq!(ctrl(&s), want);
    assert_eq!(
        in_use(&s),
        (
            Some("Fake input (16000 Hz)".into()),
            Some("Fake output (16000 Hz)".into())
        )
    );
    assert_eq!(errors(&s), [], "a route change is not an error");
}

#[test]
fn a_route_change_while_closed_opens_nothing() {
    let (s, os, mut p) = rig(false);
    ctrl(&s);
    os.set_rate(16_000);
    p.command(&s, PhoneCmd::RouteChanged);
    assert_eq!(os.log(), Vec::<String>::new());
    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), ["open 16000"]);
}

#[test]
fn a_fault_from_the_open_stream_reopens_and_a_stale_one_is_ignored() {
    let (s, os, mut p) = rig(true);
    os.log();
    ctrl(&s);
    let stale = lock(&os.0).faults.clone().expect("faults");
    os.raise_fault();
    p.tick(&s);
    assert_eq!(os.log(), ["close", "open 48000"]);
    assert_eq!(errors(&s), []);
    stale.raise();
    p.tick(&s);
    assert_eq!(
        os.log(),
        Vec::<String>::new(),
        "a fault from a closed stream"
    );
}

#[test]
fn a_failed_open_raises_once_and_is_retried() {
    let s = shared();
    let os = FakeOs::new(48_000);
    os.fail(Some(ErrorCode::DeviceBusy));
    let mut p = start(&s, &os, true, PhoneMode::Os);
    assert_eq!(os.log(), ["open 48000 failed"]);
    assert_eq!(errors(&s), [ErrorCode::DeviceBusy]);
    assert_eq!(ctrl(&s), ["pause"]);
    p.tick(&s);
    assert_eq!(os.log(), Vec::<String>::new(), "not before the retry time");

    std::thread::sleep(super::RETRY + Duration::from_millis(50));
    p.tick(&s);
    assert_eq!(os.log(), ["open 48000 failed"]);
    assert_eq!(errors(&s), [], "the same failure is raised once");

    // The app's next command tries at once.
    os.fail(None);
    p.command(&s, PhoneCmd::RouteChanged);
    assert_eq!(os.log(), ["open 48000"]);
    assert_eq!(ctrl(&s), OPENED);
    assert_eq!(errors(&s), []);
}

#[test]
fn a_device_rate_out_of_range_is_refused_and_closed() {
    let s = shared();
    let os = FakeOs::new(192_000);
    let _p = start(&s, &os, true, PhoneMode::Os);
    assert_eq!(os.log(), ["open 192000", "close"]);
    assert_eq!(errors(&s), [ErrorCode::UnsupportedConfig]);
    assert_eq!(ctrl(&s), ["pause"]);
}

#[test]
fn no_microphone_plays_and_keeps_the_capture_path_paused() {
    let s = shared();
    let os = FakeOs::new(48_000);
    os.set_mic(false);
    let _p = start(&s, &os, true, PhoneMode::Os);
    assert_eq!(os.log(), ["open 48000"]);
    assert_eq!(ctrl(&s), ["pause", "play"]);
    assert_eq!(errors(&s), [ErrorCode::NoPermission]);
    assert_eq!(in_use(&s), (None, Some("Fake output (48000 Hz)".into())));
}

#[test]
fn the_os_picks_the_route() {
    assert_eq!(check_selection(&DeviceSelection::default()), Ok(()));
    let sel = DeviceSelection {
        input: Some(DeviceChoice::Id(super::INPUT_ID.into())),
        output: Some(DeviceChoice::Default),
    };
    assert_eq!(check_selection(&sel), Ok(()));
    let sel = DeviceSelection {
        input: Some(DeviceChoice::Id("usb-mic".into())),
        output: None,
    };
    assert_eq!(
        check_selection(&sel).expect_err("not ours").code,
        ErrorCode::NoDevice
    );
}

#[test]
fn sonora_runs_only_what_the_os_does_not() {
    let all = ProcessingConfig::default().settings();
    let off = ProcessingConfig::off()
        .with_stage_fallback_noise_suppression(None)
        .settings();
    // The OS suppresses noise, so no stage fallback either.
    assert_eq!(OsEffects::default().remaining(all), off);
    let no_aec = OsEffects {
        echo_cancellation: false,
        ..OsEffects::default()
    };
    assert_eq!(
        no_aec.remaining(all),
        ApmSettings {
            echo_cancellation: true,
            noise_suppression: None,
            gain_control: false,
            high_pass: true,
            stage_fallback_ns: None,
        }
    );
    let none = OsEffects {
        echo_cancellation: false,
        noise_suppression: false,
        gain_control: false,
    };
    assert_eq!(none.remaining(all), all);
    // What the caller turned off stays off.
    assert_eq!(none.remaining(off), off);
}

#[test]
fn a_capture_stage_survives_the_os_effects() {
    struct Nop;
    impl crate::CaptureStage for Nop {
        fn process(&mut self, _: &mut [f32; 480]) {}
        fn latency_samples(&self) -> usize {
            0
        }
        fn reset(&mut self) {}
    }
    let cfg = EngineConfig {
        backend: Backend::TestPhone(FakeIo(FakeOs::new(48_000).io(), OsEffects::default())),
        processing: ProcessingConfig::default().with_capture_stage(Box::new(Nop)),
        ..EngineConfig::null(NullDevice::default())
    }
    .with_os_effects();
    // The OS suppresses noise, so there is no step-down floor either.
    assert_eq!(
        cfg.processing.settings(),
        ProcessingConfig::off()
            .with_stage_fallback_noise_suppression(None)
            .settings()
    );
    assert!(cfg.processing.capture_stage.is_some());
}

// ---- Raw mode and the mode switch, driven directly ----

#[test]
fn each_platform_opens_the_path_its_mode_asks_for() {
    let all = OsEffects::default();
    let none = OsEffects {
        echo_cancellation: false,
        noise_suppression: false,
        gain_control: false,
    };
    let no_agc = OsEffects {
        gain_control: false,
        ..all
    };
    // iOS: the voice processing unit in the OS's mode, as 0.2.0 opened it.
    assert_eq!(ios_unit(all, PhoneMode::Os), IosUnit::VoiceProcessing);
    assert_eq!(ios_unit(no_agc, PhoneMode::Os), IosUnit::VoiceProcessing);
    // Raw: the plain RemoteIO unit, whatever the effects say.
    assert_eq!(ios_unit(all, PhoneMode::raw()), IosUnit::RemoteIo);
    assert_eq!(ios_unit(none, PhoneMode::raw()), IosUnit::RemoteIo);
    // No OS effects in the OS's mode: RemoteIO too, sonora does it all.
    assert_eq!(ios_unit(none, PhoneMode::Os), IosUnit::RemoteIo);

    // iOS takes both of the unit's effects or none; raw takes anything.
    assert!(ios_effects_ok(all, PhoneMode::Os));
    assert!(ios_effects_ok(no_agc, PhoneMode::Os));
    assert!(ios_effects_ok(none, PhoneMode::Os));
    let half = OsEffects {
        noise_suppression: false,
        ..all
    };
    assert!(!ios_effects_ok(half, PhoneMode::Os));
    let agc_only = OsEffects {
        gain_control: true,
        ..none
    };
    assert!(!ios_effects_ok(agc_only, PhoneMode::Os));
    assert!(ios_effects_ok(half, PhoneMode::raw()));

    // Android: voice communication in the OS's mode, as 0.2.0 opened it.
    let os = android_input(PhoneMode::Os);
    assert_eq!(os.preset, AndroidPreset::VoiceCommunication);
    assert!(!os.no_session);
    // Raw: voice recognition, or unprocessed where the app says the device
    // supports it; no session, so no NoiseSuppressor can attach.
    let raw = android_input(PhoneMode::raw());
    assert_eq!(raw.preset, AndroidPreset::VoiceRecognition);
    assert!(raw.no_session);
    let unprocessed = android_input(PhoneMode::Raw(
        RawCapture::default().with_unprocessed_supported(true),
    ));
    assert_eq!(unprocessed.preset, AndroidPreset::Unprocessed);
    assert!(unprocessed.no_session);

    assert_eq!(PhoneMode::default(), PhoneMode::Os);
    assert!(!PhoneMode::Os.is_raw());
    assert_eq!(PhoneMode::raw(), PhoneMode::Raw(RawCapture::default()));
    assert!(!RawCapture::default().unprocessed_supported);
}

/// Raw mode: AEC3 (and its high-pass filter) and AGC2 on, noise suppression
/// off, the step-down floor off, whatever the caller and the OS flags say.
#[test]
fn raw_mode_runs_echo_cancellation_and_gain_control_and_no_noise_suppression() {
    let callers = [
        ProcessingConfig::default().settings(),
        ProcessingConfig::off().settings(),
        ProcessingConfig::default()
            .with_noise_suppression(Some(crate::NoiseSuppressionStrength::VeryHigh))
            .settings(),
    ];
    let flags = [
        OsEffects::default(),
        OsEffects {
            echo_cancellation: false,
            noise_suppression: false,
            gain_control: false,
        },
    ];
    for asked in callers {
        for fx in flags {
            for stage_latency in [None, Some(480)] {
                let p = Processing {
                    fx,
                    asked,
                    stage_latency,
                };
                for mode in [
                    PhoneMode::raw(),
                    PhoneMode::Raw(RawCapture::default().with_unprocessed_supported(true)),
                ] {
                    assert_eq!(p.settings(mode), RAW_PROCESSING);
                }
                assert_eq!(
                    RAW_PROCESSING,
                    ApmSettings {
                        echo_cancellation: true,
                        noise_suppression: None,
                        gain_control: true,
                        high_pass: true,
                        stage_fallback_ns: None,
                    }
                );
                // The sonora instances themselves.
                let chain = p.chain(PhoneMode::raw());
                let [pre, post, floor] = chain.sonora_configs();
                let pre = pre.expect("raw mode runs sonora");
                assert!(pre.echo_canceller.is_some(), "AEC3 on");
                assert!(pre.high_pass_filter.is_some(), "the high-pass filter on");
                assert!(pre.noise_suppression.is_none(), "no noise suppression");
                assert!(floor.is_none(), "no step-down floor");
                match stage_latency {
                    // Everything in one instance, as on desktop.
                    None => {
                        assert!(pre.gain_controller2.is_some(), "AGC2 on");
                        assert!(post.is_none());
                    }
                    // AGC2 after the stage, alone.
                    Some(_) => {
                        assert!(pre.gain_controller2.is_none());
                        let post = post.expect("AGC2 after the stage");
                        assert!(post.gain_controller2.is_some(), "AGC2 on");
                        assert!(post.echo_canceller.is_none());
                        assert!(post.noise_suppression.is_none());
                    }
                }
            }
        }
    }
}

/// The OS's mode is what 0.2.0 ran: sonora fills only what the OS leaves.
#[test]
fn the_os_mode_is_unchanged() {
    let all = ProcessingConfig::default().settings();
    for fx in [
        OsEffects::default(),
        OsEffects {
            noise_suppression: false,
            ..OsEffects::default()
        },
        OsEffects {
            echo_cancellation: false,
            noise_suppression: false,
            gain_control: false,
        },
    ] {
        let p = Processing {
            fx,
            asked: all,
            stage_latency: None,
        };
        assert_eq!(p.settings(PhoneMode::Os), fx.remaining(all));
    }
    // The OS does everything: no sonora at all.
    let chain = procs().chain(PhoneMode::Os);
    assert!(
        chain.sonora_configs().iter().all(Option::is_none),
        "{:?}",
        chain.sonora_configs()
    );
    assert_eq!(describe(chain.settings()), "none");
    // An Android device without a suppressor, with a stage: sonora's
    // suppression (and so no floor) runs, as before.
    let p = Processing {
        fx: OsEffects {
            noise_suppression: false,
            ..OsEffects::default()
        },
        asked: all,
        stage_latency: Some(480),
    };
    let chain = p.chain(PhoneMode::Os);
    let [pre, _, floor] = chain.sonora_configs();
    assert!(pre.expect("sonora").noise_suppression.is_some());
    assert!(floor.is_none());
}

#[test]
fn a_raw_start_opens_the_raw_path() {
    let (s, os, mut p) = rig_in(true, PhoneMode::raw());
    assert_eq!(os.log(), ["open raw 48000"]);
    // The engine built the raw chain itself; no swap at the start.
    let mut want = vec!["pause"];
    want.extend(OPENED);
    assert_eq!(ctrl(&s), want);
    assert_eq!(*lock(&s.phone_mode), PhoneMode::raw());
    assert_eq!(notices(&s), Vec::<String>::new());

    // A route change and a fault reopen in raw mode.
    os.set_rate(16_000);
    p.command(&s, PhoneCmd::RouteChanged);
    assert_eq!(os.log(), ["close", "open raw 16000"]);
    os.raise_fault();
    p.tick(&s);
    assert_eq!(os.log(), ["close", "open raw 16000"]);
    // So do a suspend and resume.
    p.command(&s, PhoneCmd::Suspend(true));
    p.command(&s, PhoneCmd::Suspend(false));
    assert_eq!(os.log(), ["close", "open raw 16000"]);
    assert_eq!(errors(&s), []);
}

#[test]
fn the_engine_config_starts_in_raw_mode() {
    struct Nop;
    impl crate::CaptureStage for Nop {
        fn process(&mut self, _: &mut [f32; 480]) {}
        fn latency_samples(&self) -> usize {
            480
        }
        fn reset(&mut self) {}
    }
    let cfg = EngineConfig {
        backend: Backend::TestPhone(FakeIo(FakeOs::new(48_000).io(), OsEffects::default())),
        processing: ProcessingConfig::default().with_capture_stage(Box::new(Nop)),
        ..EngineConfig::null(NullDevice::default())
    }
    .with_phone_mode(PhoneMode::raw());
    let kept = cfg.phone_processing().expect("a phone backend");
    assert_eq!(
        kept,
        Processing {
            fx: OsEffects::default(),
            asked: ProcessingConfig::default().settings(),
            stage_latency: Some(480),
        },
        "what the caller asked for is kept for a switch back"
    );
    let cfg = cfg.with_os_effects();
    assert_eq!(cfg.processing.settings(), RAW_PROCESSING);
    assert!(cfg.processing.capture_stage.is_some(), "the stage stays");
    // The default is the OS's mode.
    assert_eq!(
        EngineConfig::null(NullDevice::default()).phone_mode,
        PhoneMode::Os
    );
}

#[test]
fn switching_mode_mid_call_restarts_the_device_once_and_back() {
    let (s, os, mut p) = rig(true);
    os.log();
    ctrl(&s);
    events(&s);

    p.command(&s, PhoneCmd::Mode(PhoneMode::raw()));
    assert_eq!(os.log(), ["close", "open raw 48000"], "one restart");
    // Close, swap sonora's chain while paused, open: in that order.
    let mut want = CLOSED.to_vec();
    want.push(RAW);
    want.extend(OPENED);
    assert_eq!(ctrl(&s), want);
    let evs = events(&s);
    let change = evs
        .iter()
        .find_map(|e| match e {
            Event::PhoneMode(c) => Some(*c),
            _ => None,
        })
        .expect("a mode change event");
    assert_eq!(change.mode, PhoneMode::raw());
    let gap = change.gap.expect("the device restarted");
    assert!(gap < Duration::from_millis(500), "gap {gap:?}");
    assert!(!evs.iter().any(|e| matches!(e, Event::Error(_))), "{evs:?}");
    assert_eq!(*lock(&s.phone_mode), PhoneMode::raw());
    assert_eq!(s.mode_switches.load(Ordering::Relaxed), 1);
    assert_eq!(
        s.last_mode_gap_us.load(Ordering::Relaxed),
        gap.as_micros() as u64
    );
    assert_eq!(
        in_use(&s),
        (
            Some("Fake input (48000 Hz)".into()),
            Some("Fake output (48000 Hz)".into())
        )
    );

    // The same mode again does nothing.
    p.command(&s, PhoneCmd::Mode(PhoneMode::raw()));
    assert_eq!(os.log(), Vec::<String>::new());
    assert_eq!(ctrl(&s), Vec::<String>::new());
    assert_eq!(notices(&s), Vec::<String>::new());
    p.tick(&s);
    assert_eq!(os.log(), Vec::<String>::new());

    // Unprocessed is another raw mode: it restarts too.
    let unprocessed = PhoneMode::Raw(RawCapture::default().with_unprocessed_supported(true));
    p.command(&s, PhoneCmd::Mode(unprocessed));
    assert_eq!(os.log(), ["close", "open raw unprocessed 48000"]);

    // And back to the OS's voice processing.
    p.command(&s, PhoneCmd::Mode(PhoneMode::Os));
    assert_eq!(os.log(), ["close", "open 48000"]);
    let mut want = CLOSED.to_vec();
    want.push(RAW);
    want.extend(OPENED);
    want.extend(CLOSED);
    want.push(OS);
    want.extend(OPENED);
    assert_eq!(ctrl(&s), want);
    assert_eq!(notices(&s), ["mode raw restarted", "mode os restarted"]);
    assert_eq!(*lock(&s.phone_mode), PhoneMode::Os);
    assert_eq!(s.mode_switches.load(Ordering::Relaxed), 3);
}

#[test]
fn a_mode_change_with_the_device_closed_applies_when_it_opens() {
    let (s, os, mut p) = rig(false);
    ctrl(&s);
    p.command(&s, PhoneCmd::Mode(PhoneMode::raw()));
    assert_eq!(os.log(), Vec::<String>::new(), "nothing to restart");
    assert_eq!(ctrl(&s), [RAW]);
    assert_eq!(notices(&s), ["mode raw closed"]);
    assert_eq!(
        s.last_mode_gap_us.load(Ordering::Relaxed),
        u64::MAX,
        "no restart, no gap"
    );
    p.command(&s, PhoneCmd::Session(true));
    assert_eq!(os.log(), ["open raw 48000"]);
    assert_eq!(ctrl(&s), OPENED);

    // Suspended: the same.
    p.command(&s, PhoneCmd::Suspend(true));
    os.log();
    ctrl(&s);
    p.command(&s, PhoneCmd::Mode(PhoneMode::Os));
    assert_eq!(os.log(), Vec::<String>::new());
    assert_eq!(ctrl(&s), [OS]);
    p.command(&s, PhoneCmd::Suspend(false));
    assert_eq!(os.log(), ["open 48000"]);
    assert_eq!(s.mode_switches.load(Ordering::Relaxed), 2);
}

/// The call never loses its device to a mode change: if the new path won't
/// open, the old one (open a moment ago) comes back at once.
#[test]
fn a_mode_that_wont_open_goes_back_to_the_one_that_did() {
    let (s, os, mut p) = rig(true);
    os.log();
    ctrl(&s);
    events(&s);
    os.fail_raw(Some(ErrorCode::DeviceBusy));
    p.command(&s, PhoneCmd::Mode(PhoneMode::raw()));
    assert_eq!(
        os.log(),
        ["close", "open raw 48000 failed", "open 48000"],
        "back on the OS's path"
    );
    let mut want = CLOSED.to_vec();
    want.push(RAW);
    want.push(OS);
    want.extend(OPENED);
    assert_eq!(ctrl(&s), want, "sonora's chain goes back too");
    assert_eq!(notices(&s), ["error DeviceBusy"], "no mode change event");
    assert_eq!(*lock(&s.phone_mode), PhoneMode::Os);
    assert_eq!(s.mode_switches.load(Ordering::Relaxed), 0);
    // Nothing left to retry.
    std::thread::sleep(super::RETRY + Duration::from_millis(50));
    p.tick(&s);
    assert_eq!(os.log(), Vec::<String>::new());

    // Once the raw path opens, the switch goes through.
    os.fail_raw(None);
    p.command(&s, PhoneCmd::Mode(PhoneMode::raw()));
    assert_eq!(os.log(), ["close", "open raw 48000"]);
    assert_eq!(notices(&s), ["mode raw restarted"]);
}

// ---- On the engine's threads ----

fn phone_engine(os: &FakeOs, deferred: bool) -> AudioEngine {
    phone_engine_in(os, deferred, PhoneMode::Os)
}

fn phone_engine_in(os: &FakeOs, deferred: bool, mode: PhoneMode) -> AudioEngine {
    let cfg = EngineConfig {
        backend: Backend::TestPhone(FakeIo(os.io(), OsEffects::default())),
        // DTX off: a frame every 20 ms from the tone.
        encoder: EncoderCfg {
            dtx: false,
            ..EncoderCfg::default()
        },
        processing_poll: Duration::from_micros(500),
        ..EngineConfig::null(NullDevice::default())
    }
    .with_phone_mode(mode);
    if deferred {
        AudioEngine::start_when_session_active(cfg).expect("start")
    } else {
        AudioEngine::start(cfg).expect("start")
    }
}

/// Wait up to `within` for `n` frames; returns their timestamps.
fn frames(e: &AudioEngine, n: usize, within: Duration) -> Vec<u32> {
    let rx = e.capture_frames();
    let deadline = Instant::now() + within;
    let mut ts = Vec::new();
    while ts.len() < n {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(
            !left.is_zero(),
            "only {} of {n} frames in {within:?}",
            ts.len()
        );
        if let Some(f) = rx.recv_timeout(left) {
            ts.push(f.ts48k);
        }
    }
    ts
}

fn drain(e: &AudioEngine) -> Vec<u32> {
    let rx = e.capture_frames();
    std::iter::from_fn(|| rx.try_recv().map(|f| f.ts48k)).collect()
}

fn wait_for(what: &str, within: Duration, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn steps(ts: &[u32]) -> Vec<u32> {
    ts.windows(2).map(|w| w[1].wrapping_sub(w[0])).collect()
}

#[test]
fn engine_defers_until_the_session_is_active_and_survives_a_route_change() {
    let os = FakeOs::new(48_000).clocked();
    let e = phone_engine(&os, true);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(drain(&e), Vec::<u32>::new(), "no frames before the session");
    assert_eq!(os.log(), Vec::<String>::new());

    e.session_activated();
    let first = frames(&e, 10, Duration::from_secs(2));
    assert_eq!(first[0], 0, "the first frame keeps initial_ts48k");
    assert!(steps(&first).iter().all(|&d| d == 960), "{first:?}");
    assert_eq!(os.log(), ["open 48000"]);

    // Bluetooth hands-free mid-call: the device reopens at 16 kHz and the
    // frames go on at 48 kHz.
    os.set_rate(16_000);
    e.route_changed();
    wait_for("the 16 kHz device", Duration::from_secs(2), || {
        e.devices_in_use()
            .input
            .is_some_and(|d| d.name.contains("16000"))
    });
    let mut after = drain(&e);
    after.extend(frames(&e, 25, Duration::from_secs(2)));
    let all: Vec<u32> = first.iter().chain(after.iter()).copied().collect();
    // Time only moves forward, in whole frames; the reopen is at most a
    // short gap.
    for d in steps(&all) {
        assert!(
            d >= 960 && d.is_multiple_of(960) && d <= 960 * 20,
            "step {d} in {all:?}"
        );
    }
    assert_eq!(os.log(), ["close", "open 16000"]);
    let errs: Vec<Event> = std::iter::from_fn(|| e.events().try_recv())
        .filter(|ev| matches!(ev, Event::Error(_)))
        .collect();
    assert_eq!(errs, [], "a route change is not an error");
}

#[test]
fn engine_suspend_stops_frames_and_resume_carries_the_gap() {
    let os = FakeOs::new(48_000).clocked();
    let e = phone_engine(&os, false);
    let before = frames(&e, 10, Duration::from_secs(2));
    e.suspend();
    wait_for("the device to close", Duration::from_secs(2), || {
        e.devices_in_use().input.is_none()
    });
    let suspended_at = Instant::now();
    drain(&e);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(drain(&e), Vec::<u32>::new(), "no frames while suspended");
    e.resume();
    let after = frames(&e, 5, Duration::from_secs(2));
    let paused = suspended_at.elapsed();
    assert_eq!(os.log(), ["open 48000", "close", "open 48000"]);
    let last_before = *before.last().expect("frames");
    let jump = after[0].wrapping_sub(last_before);
    // At least the time we know it was suspended for, at most that plus a
    // second of scheduling slack.
    let lo = 500 * 48;
    let hi = (paused.as_millis() as u32 + 1000) * 48;
    assert!(
        jump >= lo && jump <= hi && jump.is_multiple_of(960),
        "jump {jump} (lo {lo}, hi {hi})"
    );
    assert!(steps(&after).iter().all(|&d| d == 960), "{after:?}");
}

#[test]
fn engine_routes_are_the_os_s_and_deferral_needs_a_phone_backend() {
    let os = FakeOs::new(48_000);
    let e = phone_engine(&os, false);
    e.set_device(DeviceSelection::default())
        .expect("the default route");
    let err = e
        .set_device(DeviceSelection {
            input: Some(DeviceChoice::Id("usb-mic".into())),
            output: None,
        })
        .expect_err("a device the OS would pick");
    assert_eq!(err.code, ErrorCode::NoDevice);
    let list = e.devices();
    assert_eq!(list.inputs.len(), 1);
    assert_eq!(list.outputs.len(), 1);

    let err = AudioEngine::start_when_session_active(EngineConfig::null(NullDevice::default()))
        .expect_err("the null device can't defer");
    assert_eq!(err.code, ErrorCode::InvalidConfig);
}

#[test]
fn engine_suspend_works_on_the_null_device_too() {
    let mut phase = 0.0f32;
    let dev = NullDevice {
        input: Some(Box::new(move |b: &mut [f32]| {
            for v in b.iter_mut() {
                *v = 0.3 * phase.sin();
                phase = (phase + 0.04) % std::f32::consts::TAU;
            }
        })),
        ..NullDevice::default()
    };
    let e = AudioEngine::start(EngineConfig {
        encoder: EncoderCfg {
            dtx: false,
            ..EncoderCfg::default()
        },
        processing: ProcessingConfig::off(),
        processing_poll: Duration::from_micros(500),
        ..EngineConfig::null(dev)
    })
    .expect("start");
    let before = frames(&e, 5, Duration::from_secs(2));
    e.suspend();
    std::thread::sleep(Duration::from_millis(100));
    let tail = drain(&e);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(drain(&e), Vec::<u32>::new(), "no frames while suspended");
    e.resume();
    let after = frames(&e, 3, Duration::from_secs(2));
    let last = *tail.last().unwrap_or(before.last().expect("frames"));
    let jump = after[0].wrapping_sub(last);
    assert!(jump >= 300 * 48 && jump.is_multiple_of(960), "jump {jump}");
}

/// Wait for the next mode change event; fails on any error.
fn mode_event(e: &AudioEngine, within: Duration) -> PhoneModeChange {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "no mode change event in {within:?}");
        match e.events().recv_timeout(left) {
            Some(Event::PhoneMode(c)) => return c,
            Some(Event::Error(err)) => panic!("error during the switch: {err:?}"),
            _ => {}
        }
    }
}

/// Mid-call, on the engine's threads: off to raw and back. The device
/// restarts once each way, frames keep coming with only the restart's gap
/// in `ts48k`, the participant and the encoder carry on, and the gap is
/// reported.
#[test]
fn engine_switches_to_raw_mid_call_and_back() {
    let os = FakeOs::new(48_000).clocked();
    let e = phone_engine(&os, false);
    e.add_participant(7);
    let first = frames(&e, 10, Duration::from_secs(2));
    assert!(steps(&first).iter().all(|&d| d == 960), "{first:?}");
    assert_eq!(e.phone_mode(), PhoneMode::Os);
    while e.events().try_recv().is_some() {}

    e.set_phone_mode(PhoneMode::raw());
    let to_raw = mode_event(&e, Duration::from_secs(2));
    assert_eq!(to_raw.mode, PhoneMode::raw());
    let gap = to_raw.gap.expect("the device restarted");
    assert!(gap < Duration::from_millis(500), "gap {gap:?}");
    assert_eq!(e.phone_mode(), PhoneMode::raw());
    let mut raw = drain(&e);
    raw.extend(frames(&e, 25, Duration::from_secs(2)));
    assert_eq!(os.log(), ["open 48000", "close", "open raw 48000"]);
    let s = e.stats();
    assert_eq!(s.phone_mode_switches, 1);
    assert_eq!(
        s.last_phone_mode_gap,
        Some(Duration::from_micros(gap.as_micros() as u64))
    );
    assert_eq!(
        s.participants.iter().map(|p| p.0).collect::<Vec<_>>(),
        [7],
        "the participant is still there"
    );

    e.set_phone_mode(PhoneMode::Os);
    let back = mode_event(&e, Duration::from_secs(2));
    assert_eq!(back.mode, PhoneMode::Os);
    assert!(back.gap.is_some());
    let mut os_again = drain(&e);
    os_again.extend(frames(&e, 25, Duration::from_secs(2)));
    assert_eq!(os.log(), ["close", "open 48000"]);
    assert_eq!(e.stats().phone_mode_switches, 2);
    assert_eq!(e.phone_mode(), PhoneMode::Os);

    // Time only moves forward, in whole frames; each switch is at most a
    // short gap (it reads as a pause, like the end of a DTX gap).
    let all: Vec<u32> = first
        .iter()
        .chain(raw.iter())
        .chain(os_again.iter())
        .copied()
        .collect();
    for d in steps(&all) {
        assert!(
            d >= 960 && d.is_multiple_of(960) && d <= 960 * 20,
            "step {d} in {all:?}"
        );
    }
    let errs: Vec<Event> = std::iter::from_fn(|| e.events().try_recv())
        .filter(|ev| matches!(ev, Event::Error(_)))
        .collect();
    assert_eq!(errs, [], "a mode change is not an error");
}

#[test]
fn engine_starts_in_raw_mode_and_switching_before_the_session_costs_no_restart() {
    let os = FakeOs::new(48_000).clocked();
    let e = phone_engine_in(&os, false, PhoneMode::raw());
    frames(&e, 5, Duration::from_secs(2));
    assert_eq!(os.log(), ["open raw 48000"]);
    assert_eq!(e.phone_mode(), PhoneMode::raw());
    assert_eq!(e.stats().phone_mode_switches, 0);
    drop(e);

    // With CallKit: start deferred, pick the mode, then the session opens
    // the device once, in that mode.
    let os = FakeOs::new(48_000).clocked();
    let e = phone_engine(&os, true);
    e.set_phone_mode(PhoneMode::raw());
    let c = mode_event(&e, Duration::from_secs(2));
    assert_eq!(c.gap, None, "no device was open");
    e.session_activated();
    frames(&e, 5, Duration::from_secs(2));
    assert_eq!(os.log(), ["open raw 48000"]);
    assert_eq!(e.stats().last_phone_mode_gap, None);
}
