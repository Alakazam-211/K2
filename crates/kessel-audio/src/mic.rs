//! The microphone permission gate.
//!
//! macOS doesn't fail an input stream the person refused: the stream opens
//! and delivers silence. So the system backend asks this gate before it
//! touches an input device. Every other OS has no separate check here: its
//! refusal comes back as an error from opening the device.
//!
//! The decisions are plain code over the [`MicOs`] trait, so they run with a
//! fake on any OS. The only OS calls are in `macos` at the bottom.
//!
//! A prompt can stay up for as long as the person likes, and the control
//! thread that calls this also delivers events and frees memory, so asking
//! never blocks: the answer lands in a [`Reply`] the control thread looks at
//! on its next tick.

use crate::error::{AudioError, ErrorCode};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// What the OS says about this app and the microphone. Only macOS's gate
/// reports anything but `Granted` outside tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) enum MicStatus {
    Granted,
    /// The person hasn't been asked yet.
    NotDetermined,
    /// The person said no (or turned it off in the system settings).
    Denied,
    /// Policy forbids it (device management, parental controls); the person
    /// can't change it.
    Restricted,
}

/// Where the answer to [`MicOs::ask`] lands. Any thread may send it.
#[derive(Clone, Default)]
pub(crate) struct Reply(Arc<AtomicU8>);

impl Reply {
    const YES: u8 = 1;
    const NO: u8 = 2;

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn send(&self, granted: bool) {
        let v = if granted { Self::YES } else { Self::NO };
        self.0.store(v, Ordering::Release);
    }

    fn get(&self) -> Option<bool> {
        match self.0.load(Ordering::Acquire) {
            Self::YES => Some(true),
            Self::NO => Some(false),
            _ => None,
        }
    }
}

/// The OS's side of the gate.
pub(crate) trait MicOs {
    /// The current status. Must not prompt.
    fn status(&self) -> MicStatus;
    /// Whether asking is safe. On macOS the OS kills an app that asks
    /// without a usage description, and a library must not take its host
    /// down with it.
    fn can_ask(&self) -> bool;
    /// Show the OS prompt and return at once; the answer goes to `reply`.
    fn ask(&self, reply: Reply);
}

/// May an input device be opened now?
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Open,
    /// The prompt is up. Nothing opens until the person answers.
    Waiting,
    Refused(AudioError),
}

pub(crate) struct MicGate {
    os: Box<dyn MicOs>,
    reply: Reply,
    asked: bool,
    answer_seen: bool,
    last: Option<MicStatus>,
}

fn refused(detail: &str) -> Gate {
    Gate::Refused(AudioError::new(ErrorCode::NoPermission, detail))
}

impl MicGate {
    pub(crate) fn new(os: Box<dyn MicOs>) -> MicGate {
        MicGate {
            os,
            reply: Reply::default(),
            asked: false,
            answer_seen: false,
            last: None,
        }
    }

    /// Decide, asking the person if they have never been asked. Asks at most
    /// once in this gate's life.
    pub(crate) fn check(&mut self) -> Gate {
        let status = self.os.status();
        self.last = Some(status);
        match status {
            MicStatus::Granted => Gate::Open,
            MicStatus::Denied => {
                refused("microphone access is turned off for this app in the system settings")
            }
            MicStatus::Restricted => {
                refused("microphone access is restricted on this device by policy")
            }
            MicStatus::NotDetermined => match self.reply.get() {
                Some(true) => Gate::Open,
                Some(false) => refused("the person declined microphone access"),
                None if self.asked => Gate::Waiting,
                None if !self.os.can_ask() => refused(
                    "microphone access was never granted, and the app can't ask: \
                     its Info.plist has no NSMicrophoneUsageDescription",
                ),
                None => {
                    self.asked = true;
                    self.os.ask(self.reply.clone());
                    Gate::Waiting
                }
            },
        }
    }

    /// True once, when the person has answered the prompt.
    pub(crate) fn answered(&mut self) -> bool {
        if self.asked && !self.answer_seen && self.reply.get().is_some() {
            self.answer_seen = true;
            return true;
        }
        false
    }

    /// The status moved since the last [`MicGate::check`] (the person
    /// changed it in the system settings mid-call).
    pub(crate) fn status_changed(&self) -> bool {
        self.last != Some(self.os.status())
    }
}

/// This OS's gate.
pub(crate) fn system_os() -> Box<dyn MicOs> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::CaptureDeviceAuth)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(NoCheck)
    }
}

/// Elsewhere a refusal comes back from opening the device.
#[cfg(not(target_os = "macos"))]
struct NoCheck;

#[cfg(not(target_os = "macos"))]
impl MicOs for NoCheck {
    fn status(&self) -> MicStatus {
        MicStatus::Granted
    }
    fn can_ask(&self) -> bool {
        false
    }
    fn ask(&self, _reply: Reply) {}
}

/// AVCaptureDevice's authorization calls, reached through the Objective-C
/// runtime. With the phone backends' OS calls, the crate's only `unsafe`
/// outside tests.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos {
    use super::{MicOs, MicStatus, Reply};
    use block2::RcBlock;
    use objc2::rc::autoreleasepool;
    use objc2::runtime::{AnyObject, Bool};
    use objc2::{class, msg_send};

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: *const AnyObject;
    }

    /// `AVAuthorizationStatus` to ours. A value this code doesn't know is
    /// reported as a refusal: loud rather than silent.
    fn from_raw(raw: isize) -> MicStatus {
        match raw {
            0 => MicStatus::NotDetermined,
            1 => MicStatus::Restricted,
            3 => MicStatus::Granted,
            _ => MicStatus::Denied,
        }
    }

    pub(super) struct CaptureDeviceAuth;

    impl MicOs for CaptureDeviceAuth {
        fn status(&self) -> MicStatus {
            // SAFETY: a class method taking an AVMediaType (a framework
            // constant, linked above) and returning an NSInteger. It doesn't
            // prompt, and any thread may call it.
            let raw: isize = unsafe {
                msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
            };
            from_raw(raw)
        }

        fn can_ask(&self) -> bool {
            // The control thread has no autorelease pool of its own.
            autoreleasepool(|_| {
                // SAFETY: Foundation class and instance methods with the
                // signatures Apple documents; the C string is NUL-terminated
                // and outlives the call, and each object is checked for nil.
                // They return autoreleased objects, which this pool keeps
                // alive, so raw pointers are enough and nothing is retained.
                unsafe {
                    let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
                    let key: *mut AnyObject = msg_send![
                        class!(NSString),
                        stringWithUTF8String: c"NSMicrophoneUsageDescription".as_ptr()
                    ];
                    if bundle.is_null() || key.is_null() {
                        return false;
                    }
                    let value: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: key];
                    !value.is_null()
                }
            })
        }

        fn ask(&self, reply: Reply) {
            let done = RcBlock::new(move |granted: Bool| reply.send(granted.as_bool()));
            autoreleasepool(|_| {
                // SAFETY: a class method taking an AVMediaType and a
                // `void (^)(BOOL)` block, which it copies. It returns at once;
                // the block runs later on a queue of the OS's choosing and
                // only stores to an atomic.
                unsafe {
                    let _: () = msg_send![
                        class!(AVCaptureDevice),
                        requestAccessForMediaType: AVMediaTypeAudio,
                        completionHandler: &*done
                    ];
                }
            });
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{Duration, Instant};

        #[test]
        fn raw_statuses() {
            assert_eq!(from_raw(0), MicStatus::NotDetermined);
            assert_eq!(from_raw(1), MicStatus::Restricted);
            assert_eq!(from_raw(2), MicStatus::Denied);
            assert_eq!(from_raw(3), MicStatus::Granted);
            assert_eq!(from_raw(99), MicStatus::Denied);
        }

        /// The real OS calls that never prompt. A test binary has no
        /// Info.plist, so this is also the "can't ask" path.
        #[test]
        fn status_and_usage_check_run_without_a_prompt() {
            let os = CaptureDeviceAuth;
            eprintln!("[mic] status {:?}", os.status());
            assert!(!os.can_ask(), "a test binary has no usage description");
        }

        /// The whole engine with the microphone really refused. By hand, on a
        /// Mac where this process's microphone access is off. It starts the
        /// real output device but can't open the input:
        /// `cargo test --lib real_denied -- --ignored --nocapture`.
        #[test]
        #[ignore = "needs the microphone refused for this process; starts the real output device"]
        fn real_denied_mic_raises_no_permission() {
            use crate::{
                AudioEngine, DeviceChoice, DeviceSelection, EngineConfig, ErrorCode, Event,
            };
            assert_eq!(
                CaptureDeviceAuth.status(),
                MicStatus::Denied,
                "refuse the microphone first"
            );
            let engine = AudioEngine::start(EngineConfig::default()).expect("start");
            let events = engine.events();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut raised = false;
            while !raised && Instant::now() < deadline {
                if let Some(Event::Error(e)) = events.recv_timeout(Duration::from_millis(100)) {
                    eprintln!("[mic] event: {e}");
                    raised = e.code == ErrorCode::NoPermission;
                }
            }
            assert!(raised, "no no_permission event in 5 s");
            assert_eq!(engine.devices_in_use().input, None);
            let sel = DeviceSelection {
                input: Some(DeviceChoice::Default),
                output: None,
            };
            let e = engine.set_device(sel).expect_err("the switch must fail");
            assert_eq!(e.code, ErrorCode::NoPermission);
            assert!(
                engine.capture_frames().try_recv().is_none(),
                "frames from a refused microphone"
            );
        }

        /// The real prompt call. Needs a usage description linked into the
        /// test binary, and shows a prompt, so it only runs by hand:
        /// `RUSTFLAGS="-C link-arg=-Wl,-sectcreate,__TEXT,__info_plist,<Info.plist>"
        /// cargo test --lib real_ask -- --ignored --nocapture`.
        #[test]
        #[ignore = "shows the OS prompt; needs a usage description linked in"]
        fn real_ask_returns_at_once_and_calls_back() {
            let os = CaptureDeviceAuth;
            assert!(
                os.can_ask(),
                "link an Info.plist with the usage description"
            );
            let reply = Reply::default();
            let t = Instant::now();
            os.ask(reply.clone());
            assert!(
                t.elapsed() < Duration::from_secs(2),
                "ask blocked for {:?}",
                t.elapsed()
            );
            let deadline = Instant::now() + Duration::from_secs(60);
            while reply.get().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            eprintln!("[mic] answer {:?}, status {:?}", reply.get(), os.status());
            assert!(reply.get().is_some(), "no answer in 60 s");
        }
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    struct State {
        status: MicStatus,
        can_ask: bool,
        asks: u32,
        reply: Option<Reply>,
    }

    /// A scripted OS. Clones share their state.
    #[derive(Clone)]
    pub(crate) struct FakeOs(Arc<Mutex<State>>);

    impl FakeOs {
        pub(crate) fn new(status: MicStatus) -> FakeOs {
            FakeOs(Arc::new(Mutex::new(State {
                status,
                can_ask: true,
                asks: 0,
                reply: None,
            })))
        }

        fn state(&self) -> std::sync::MutexGuard<'_, State> {
            self.0.lock().expect("fake OS lock")
        }

        pub(crate) fn no_usage_description(self) -> FakeOs {
            self.state().can_ask = false;
            self
        }

        pub(crate) fn set_status(&self, s: MicStatus) {
            self.state().status = s;
        }

        pub(crate) fn asks(&self) -> u32 {
            self.state().asks
        }

        /// The person answers the prompt: the status moves, then the
        /// callback runs, as on macOS.
        pub(crate) fn answer(&self, granted: bool) {
            let mut g = self.state();
            g.status = if granted {
                MicStatus::Granted
            } else {
                MicStatus::Denied
            };
            g.reply.as_ref().expect("nobody asked").send(granted);
        }
    }

    impl MicOs for FakeOs {
        fn status(&self) -> MicStatus {
            self.state().status
        }
        fn can_ask(&self) -> bool {
            self.state().can_ask
        }
        fn ask(&self, reply: Reply) {
            let mut g = self.state();
            g.asks += 1;
            g.reply = Some(reply);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeOs;
    use super::*;

    fn gate(os: &FakeOs) -> MicGate {
        MicGate::new(Box::new(os.clone()))
    }

    fn assert_refused(g: Gate) {
        match g {
            Gate::Refused(e) => {
                assert_eq!(e.code, ErrorCode::NoPermission);
                assert_eq!(e.code.as_str(), "no_permission");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn granted_opens_without_asking() {
        let os = FakeOs::new(MicStatus::Granted);
        assert_eq!(gate(&os).check(), Gate::Open);
        assert_eq!(os.asks(), 0);
    }

    #[test]
    fn denied_and_restricted_refuse_without_asking() {
        for status in [MicStatus::Denied, MicStatus::Restricted] {
            let os = FakeOs::new(status);
            assert_refused(gate(&os).check());
            assert_eq!(os.asks(), 0, "{status:?}");
        }
    }

    #[test]
    fn not_determined_asks_once_and_waits() {
        let os = FakeOs::new(MicStatus::NotDetermined);
        let mut g = gate(&os);
        assert_eq!(g.check(), Gate::Waiting);
        assert_eq!(g.check(), Gate::Waiting);
        assert_eq!(os.asks(), 1, "a second check must not ask again");
        assert!(!g.answered());
    }

    #[test]
    fn a_yes_opens_and_is_reported_once() {
        let os = FakeOs::new(MicStatus::NotDetermined);
        let mut g = gate(&os);
        g.check();
        os.answer(true);
        assert!(g.answered());
        assert!(!g.answered(), "the answer is reported once");
        assert_eq!(g.check(), Gate::Open);
    }

    #[test]
    fn a_no_refuses_and_is_never_asked_again() {
        let os = FakeOs::new(MicStatus::NotDetermined);
        let mut g = gate(&os);
        g.check();
        os.answer(false);
        assert!(g.answered());
        assert_refused(g.check());
        assert_eq!(os.asks(), 1);
    }

    /// If the callback beats the status, the answer still decides, and a
    /// "no" can't turn into a second prompt.
    #[test]
    fn the_answer_decides_while_the_status_lags() {
        for (granted, opens) in [(true, true), (false, false)] {
            let os = FakeOs::new(MicStatus::NotDetermined);
            let mut g = gate(&os);
            g.check();
            os.answer(granted);
            os.set_status(MicStatus::NotDetermined);
            match g.check() {
                Gate::Open => assert!(opens),
                other => {
                    assert!(!opens, "{other:?}");
                    assert_refused(other);
                }
            }
            assert_eq!(os.asks(), 1);
        }
    }

    #[test]
    fn without_a_usage_description_it_refuses_instead_of_asking() {
        let os = FakeOs::new(MicStatus::NotDetermined).no_usage_description();
        let mut g = gate(&os);
        match g.check() {
            Gate::Refused(e) => assert!(e.detail.contains("NSMicrophoneUsageDescription"), "{e}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(os.asks(), 0, "asking without one kills the app on macOS");
    }

    #[test]
    fn status_changes_are_noticed() {
        let os = FakeOs::new(MicStatus::Denied);
        let mut g = gate(&os);
        assert!(g.status_changed(), "nothing checked yet");
        assert_refused(g.check());
        assert!(!g.status_changed());
        os.set_status(MicStatus::Granted);
        assert!(g.status_changed());
        assert_eq!(g.check(), Gate::Open);
        assert!(!g.status_changed());
    }

    #[test]
    fn no_answer_means_nothing_to_report() {
        let os = FakeOs::new(MicStatus::Granted);
        let mut g = gate(&os);
        g.check();
        assert!(!g.answered());
    }
}
