//! The phone backend's state machine on a fake OS: deferred start, session
//! activate and deactivate, suspend and resume, route changes, faults and
//! retries. The state machine tests drive it directly; the engine tests run
//! it on the engine's real threads with a fake device that plays a clock.

use super::{check_selection, Ends, Faults, Opened, OsEffects, Phone, PhoneIo};
use crate::apm::ProcessingConfig;
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

    fn open(&mut self, ends: Ends) -> Result<Opened, AudioError> {
        let mut st = lock(&self.0 .0);
        let rate = st.rate;
        if let Some(code) = st.fail {
            st.log.push(format!("open {rate} failed"));
            return Err(AudioError::new(code, "scripted failure"));
        }
        st.log.push(format!("open {rate}"));
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
            _ => panic!("unexpected control message"),
        });
    }
    out
}

fn errors(shared: &Shared) -> Vec<ErrorCode> {
    let mut out = Vec::new();
    while let Some(e) = shared.events.pop() {
        if let Event::Error(e) = e {
            out.push(e.code);
        }
    }
    out
}

fn in_use(shared: &Shared) -> (Option<String>, Option<String>) {
    let u = lock(&shared.in_use).clone();
    (u.input.map(|d| d.name), u.output.map(|d| d.name))
}

const OPENED: [&str; 3] = ["capture", "play", "unpause"];
const CLOSED: [&str; 3] = ["pause", "capture off", "play off"];

fn rig(session_active: bool) -> (Arc<Shared>, FakeOs, Phone) {
    let s = shared();
    let os = FakeOs::new(48_000);
    let p = Phone::start(&s, os.io(), session_active);
    (s, os, p)
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
    let mut p = Phone::start(&s, os.io(), true);
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
    let _p = Phone::start(&s, os.io(), true);
    assert_eq!(os.log(), ["open 192000", "close"]);
    assert_eq!(errors(&s), [ErrorCode::UnsupportedConfig]);
    assert_eq!(ctrl(&s), ["pause"]);
}

#[test]
fn no_microphone_plays_and_keeps_the_capture_path_paused() {
    let s = shared();
    let os = FakeOs::new(48_000);
    os.set_mic(false);
    let _p = Phone::start(&s, os.io(), true);
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
    let all = ProcessingConfig::default();
    assert_eq!(
        OsEffects::default().remaining(&all),
        ProcessingConfig::off()
    );
    let no_aec = OsEffects {
        echo_cancellation: false,
        ..OsEffects::default()
    };
    assert_eq!(
        no_aec.remaining(&all),
        ProcessingConfig {
            echo_cancellation: true,
            noise_suppression: None,
            gain_control: false,
            high_pass: true,
        }
    );
    let none = OsEffects {
        echo_cancellation: false,
        noise_suppression: false,
        gain_control: false,
    };
    assert_eq!(none.remaining(&all), all);
    // What the caller turned off stays off.
    assert_eq!(
        none.remaining(&ProcessingConfig::off()),
        ProcessingConfig::off()
    );
}

// ---- On the engine's threads ----

fn phone_engine(os: &FakeOs, deferred: bool) -> AudioEngine {
    let cfg = EngineConfig {
        backend: Backend::TestPhone(FakeIo(os.io(), OsEffects::default())),
        // DTX off: a frame every 20 ms from the tone.
        encoder: EncoderCfg {
            dtx: false,
            ..EncoderCfg::default()
        },
        processing_poll: Duration::from_micros(500),
        ..EngineConfig::null(NullDevice::default())
    };
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
