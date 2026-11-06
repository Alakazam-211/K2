//! The running engine: its threads, queues and the public handle.
//!
//! Threads:
//! - **device threads** (the OS's audio callbacks, or the null device's
//!   clock thread) move samples between the device and two lock-free rings;
//! - **the processing thread** (`kessel-audio-dsp`) owns [`Core`]: it
//!   resamples, runs echo cancellation, encodes, runs the jitter buffers,
//!   decodes and mixes. It never takes a lock it could wait on (snapshots
//!   are published with `try_lock`) and never blocks;
//! - **the control thread** (`kessel-audio-ctl`) opens and closes devices,
//!   watches for hot-plug, turns snapshots into events, and frees memory the
//!   processing thread hands back.

use crate::device::{CaptureIo, IoCounters, PlayIo};
use crate::error::{AudioError, ErrorCode};
use crate::events::{
    DeviceChoice, DeviceList, DeviceSelection, DevicesInUse, Event, EventRx, Levels,
};
use crate::frame::{EncodedFrame, EncoderCfg, FrameRx, RecvFrame, BLOCK_SAMPLES};
use crate::pipeline::{Core, CoreConfig, Participant, Pause};
use crate::stats::Stats;
use crate::{Backend, EngineConfig};
use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub(crate) enum Ctrl {
    Capture(bool),
    Deafen(bool),
    Encoder(EncoderCfg),
    Add(Box<Participant>),
    Remove(u64),
    CaptureIo(Option<Box<CaptureIo>>),
    PlayIo(Option<Box<PlayIo>>),
    Pause { why: Pause, on: bool },
}

/// From the caller to the phone backend (on the control thread). Other
/// backends ignore them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PhoneCmd {
    /// The OS's audio session became active (`true`) or inactive.
    #[cfg_attr(
        not(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        )),
        allow(dead_code)
    )]
    Session(bool),
    /// The call was suspended (`true`) or resumed.
    #[cfg_attr(
        not(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        )),
        allow(dead_code)
    )]
    Suspend(bool),
    /// The OS's audio route changed.
    #[cfg_attr(
        not(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        )),
        allow(dead_code)
    )]
    RouteChanged,
}

pub(crate) enum RecvItem {
    Frame(RecvFrame),
    Lost { participant: u64, seq: u16 },
}

/// Memory the processing thread hands to the control thread to free.
pub(crate) enum Garbage {
    Participant(#[allow(dead_code)] Box<Participant>),
    Capture(#[allow(dead_code)] Box<CaptureIo>),
    Play(#[allow(dead_code)] Box<PlayIo>),
}

pub(crate) struct DevRequest {
    pub(crate) sel: DeviceSelection,
    pub(crate) reply: SyncSender<Result<(), AudioError>>,
}

#[derive(Default)]
pub(crate) struct Snapshot {
    pub(crate) version: u64,
    pub(crate) levels: Levels,
    pub(crate) stats: Stats,
}

pub(crate) struct Shared {
    pub(crate) ctrl: ArrayQueue<Ctrl>,
    pub(crate) recv: ArrayQueue<RecvItem>,
    pub(crate) frames: Arc<ArrayQueue<EncodedFrame>>,
    pub(crate) events: Arc<ArrayQueue<Event>>,
    pub(crate) snapshot: Mutex<Snapshot>,
    pub(crate) devices: Mutex<DeviceList>,
    pub(crate) in_use: Mutex<DevicesInUse>,
    pub(crate) garbage: ArrayQueue<Garbage>,
    pub(crate) io: IoCounters,
    pub(crate) stop: AtomicBool,
    pub(crate) recv_drops: AtomicU64,
    pub(crate) dev_req: ArrayQueue<DevRequest>,
    pub(crate) phone_cmd: ArrayQueue<PhoneCmd>,
    pub(crate) core_cfg: CoreConfig,
    pub(crate) playout_buffer_ms: u32,
    #[cfg_attr(not(feature = "cpal"), allow(dead_code))]
    pub(crate) device_poll: Duration,
}

/// Lock a mutex, ignoring poisoning (the data is plain state; a panic
/// elsewhere doesn't make it unusable).
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    pub(crate) fn new(cfg: &EngineConfig) -> Arc<Shared> {
        let core_cfg = CoreConfig {
            processing: cfg.processing.clone(),
            encoder: cfg.encoder,
            jitter: cfg.jitter.clone(),
            initial_ts48k: cfg.initial_ts48k,
            levels_interval_ms: cfg.levels_interval_ms,
            speaking_threshold_dbov: cfg.speaking_threshold_dbov,
            max_participants: cfg.max_participants,
        };
        let mut snapshot = Snapshot::default();
        snapshot.levels.participants.reserve(cfg.max_participants);
        snapshot.stats.participants.reserve(cfg.max_participants);
        Arc::new(Shared {
            ctrl: ArrayQueue::new(256),
            recv: ArrayQueue::new(cfg.recv_queue_frames),
            frames: Arc::new(ArrayQueue::new(cfg.capture_queue_frames)),
            events: Arc::new(ArrayQueue::new(256)),
            snapshot: Mutex::new(snapshot),
            devices: Mutex::new(DeviceList::default()),
            in_use: Mutex::new(DevicesInUse::default()),
            garbage: ArrayQueue::new(1024),
            io: IoCounters::default(),
            stop: AtomicBool::new(false),
            recv_drops: AtomicU64::new(0),
            dev_req: ArrayQueue::new(16),
            phone_cmd: ArrayQueue::new(64),
            core_cfg,
            playout_buffer_ms: cfg.playout_buffer_ms,
            device_poll: cfg.device_poll_interval,
        })
    }

    pub(crate) fn emit(&self, e: Event) {
        self.events.force_push(e);
    }

    /// Queue a control message. The queue is large and drained every few
    /// milliseconds; if it is full, wait briefly on the caller's thread.
    pub(crate) fn send_ctrl(&self, mut c: Ctrl) -> bool {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.ctrl.push(c) {
                Ok(()) => return true,
                Err(back) => {
                    if Instant::now() >= deadline || self.stop.load(Ordering::Acquire) {
                        return false;
                    }
                    c = back;
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
    }

    pub(crate) fn set_in_use(&self, f: impl FnOnce(&mut DevicesInUse)) {
        let now = {
            let mut g = lock(&self.in_use);
            f(&mut g);
            g.clone()
        };
        self.emit(Event::DevicesInUse(now));
    }
}

/// A running audio engine. `Send + Sync`: share it (for example in an
/// `Arc`) between the transport's tasks. Dropping it stops the threads and
/// closes the devices.
pub struct AudioEngine {
    shared: Arc<Shared>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for AudioEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioEngine").finish_non_exhaustive()
    }
}

impl AudioEngine {
    /// Start the engine: open the devices and start the threads.
    ///
    /// Errors only for an invalid configuration or if the threads can't
    /// start. Device problems (no microphone, no permission, a busy device)
    /// don't stop the engine: they arrive as [`Event::Error`] on
    /// [`AudioEngine::events`], and the engine runs with whatever devices
    /// it could open.
    ///
    /// On macOS the input device opens only once the person has allowed the
    /// microphone. If they've never been asked, this shows the OS prompt
    /// without waiting for it: playback starts at once, and the input opens
    /// when they say yes ([`Event::DevicesInUse`]) or
    /// [`crate::ErrorCode::NoPermission`] arrives when they say no. The app
    /// needs `NSMicrophoneUsageDescription` in its Info.plist; without it the
    /// OS would kill the app for asking, so the engine raises
    /// [`crate::ErrorCode::NoPermission`] instead.
    pub fn start(cfg: EngineConfig) -> Result<AudioEngine, AudioError> {
        Self::start_inner(cfg, true)
    }

    /// Like [`AudioEngine::start`], but the device opens only once
    /// [`AudioEngine::session_activated`] says the OS's audio session is live.
    /// For iOS with CallKit: start the engine when the call starts, and call
    /// `session_activated` from `provider(_:didActivate:)`.
    ///
    /// Until then the engine runs without a device: no frames, and received
    /// frames are decoded on the wall clock so the jitter buffers are current
    /// when the device opens. Only `Backend::OsVoiceProcessing` defers; any
    /// other backend is [`ErrorCode::InvalidConfig`].
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub fn start_when_session_active(cfg: EngineConfig) -> Result<AudioEngine, AudioError> {
        if !cfg.backend.is_phone() {
            return Err(AudioError::new(
                ErrorCode::InvalidConfig,
                "start_when_session_active needs Backend::OsVoiceProcessing",
            ));
        }
        Self::start_inner(cfg, false)
    }

    fn start_inner(cfg: EngineConfig, session_active: bool) -> Result<AudioEngine, AudioError> {
        cfg.validate()?;
        // The OS's voice processing replaces sonora's where it can.
        #[cfg(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        ))]
        let cfg = cfg.with_os_effects();
        #[cfg(not(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        )))]
        let _ = session_active;
        let shared = Shared::new(&cfg);
        let core_cfg = shared.core_cfg.clone();
        let engine = AudioEngine {
            shared: shared.clone(),
            threads: Mutex::new(Vec::new()),
        };

        // The processing thread builds the core itself and reports back.
        let (ready_tx, ready_rx) = sync_channel::<Result<(), AudioError>>(1);
        let poll = cfg.processing_poll;
        let s = shared.clone();
        let dsp = std::thread::Builder::new()
            .name("kessel-audio-dsp".into())
            .spawn(move || match Core::new(core_cfg) {
                Ok(core) => {
                    let _ = ready_tx.send(Ok(()));
                    dsp_main(s, core, poll);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| AudioError::new(ErrorCode::Backend, format!("spawn: {e}")))?;
        lock(&engine.threads).push(dsp);
        match ready_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(AudioError::new(
                    ErrorCode::Backend,
                    "processing thread exited during start-up",
                ))
            }
        }

        let ctl_backend = match cfg.backend {
            Backend::Null(dev) => {
                let h = crate::null::start(&shared, dev)?;
                lock(&engine.threads).push(h);
                CtlBackend::Null
            }
            #[cfg(feature = "cpal")]
            Backend::System { input, output } => CtlBackend::System { input, output },
            #[cfg(all(feature = "os-voice", any(target_os = "ios", target_os = "android")))]
            Backend::OsVoiceProcessing(c) => CtlBackend::Phone {
                io: crate::phone::system_io(&c),
                session_active,
            },
            #[cfg(test)]
            Backend::TestPhone(f) => CtlBackend::Phone {
                io: f.0,
                session_active,
            },
        };
        let s = shared.clone();
        let ctl = std::thread::Builder::new()
            .name("kessel-audio-ctl".into())
            .spawn(move || ctl_main(s, ctl_backend))
            .map_err(|e| AudioError::new(ErrorCode::Backend, format!("spawn: {e}")))?;
        lock(&engine.threads).push(ctl);
        Ok(engine)
    }

    /// The encoded-frame queue (clones share it). See [`EncodedFrame`] for
    /// the timestamp rules.
    pub fn capture_frames(&self) -> FrameRx {
        FrameRx {
            q: self.shared.frames.clone(),
        }
    }

    /// Mute (`false`) or unmute (`true`). Muted, no frames are produced;
    /// echo cancellation keeps running so it stays converged, and `ts48k`
    /// keeps advancing.
    pub fn set_capture(&self, enabled: bool) {
        self.shared.send_ctrl(Ctrl::Capture(enabled));
    }

    /// Change the encoder settings; applies from the next frame without
    /// resetting the codec.
    pub fn set_encoder(&self, cfg: EncoderCfg) {
        self.shared.send_ctrl(Ctrl::Encoder(cfg));
    }

    /// Hand over a received frame (already decrypted and verified). Never
    /// blocks: if the receive queue is full the frame is dropped and
    /// counted in [`Stats::recv_queue_drops`].
    pub fn push(&self, f: RecvFrame) {
        if self.shared.recv.push(RecvItem::Frame(f)).is_err() {
            self.shared.recv_drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The transport knows packet `seq` from `participant` is gone (for
    /// example a NACK gave up). Lets the jitter buffer conceal it at once
    /// instead of waiting.
    pub fn lost(&self, participant: u64, seq: u16) {
        if self
            .shared
            .recv
            .push(RecvItem::Lost { participant, seq })
            .is_err()
        {
            self.shared.recv_drops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Start receiving a participant. Frames for ids that were never added
    /// are dropped (and counted). Adding an id twice is a no-op.
    pub fn add_participant(&self, id: u64) {
        match Participant::new(id, &self.shared.core_cfg) {
            Ok(p) => {
                self.shared.send_ctrl(Ctrl::Add(p));
            }
            Err(e) => self.shared.emit(Event::Error(e)),
        }
    }

    /// Stop receiving a participant and free its state.
    pub fn remove_participant(&self, id: u64) {
        self.shared.send_ctrl(Ctrl::Remove(id));
    }

    /// Stop (`true`) or resume playout. Deafened, the output and the echo
    /// reference are silent; participants are still decoded so their
    /// levels and stats stay live.
    pub fn set_deafened(&self, on: bool) {
        self.shared.send_ctrl(Ctrl::Deafen(on));
    }

    /// Suspend the call: for hold (a cellular call), an interruption, or
    /// lost audio focus. No frames are produced and playback is silent until
    /// [`AudioEngine::resume`]; the phone backend also stops its device.
    /// Participants are still decoded, so the jitter buffers stay current.
    ///
    /// The far end sees a pause, not loss: if you were speaking, one frame of
    /// encoded silence (marked [`crate::EncodedFrame::dtx`]) ends the
    /// talkspurt, and the first frame after `resume` has a `ts48k` advanced
    /// by the suspended time (whole frames), like the end of a DTX gap. Keep
    /// the transport's sequence numbers running as usual.
    ///
    /// Works with every backend. Suspending twice is the same as once.
    pub fn suspend(&self) {
        self.set_suspended(true);
    }

    /// End a [`AudioEngine::suspend`].
    pub fn resume(&self) {
        self.set_suspended(false);
    }

    fn set_suspended(&self, on: bool) {
        self.shared.send_ctrl(Ctrl::Pause {
            why: Pause::Caller,
            on,
        });
        self.phone(PhoneCmd::Suspend(on));
    }

    fn phone(&self, cmd: PhoneCmd) {
        // The control thread drains this every 10 ms; it only fills up if
        // that thread has stopped.
        if self.shared.phone_cmd.push(cmd).is_err() {
            self.shared.emit(Event::Error(AudioError::new(
                ErrorCode::Backend,
                "the control thread is not taking commands",
            )));
        }
    }

    /// The OS's audio session is live: open the device. Call it from
    /// CallKit's `provider(_:didActivate:)` (iOS). Never blocks; the device
    /// opens on the control thread a few milliseconds later
    /// ([`Event::DevicesInUse`]).
    ///
    /// The engine never configures or activates the session itself. Only the
    /// phone backend uses this; others ignore it.
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub fn session_activated(&self) {
        self.phone(PhoneCmd::Session(true));
    }

    /// The OS's audio session is no longer live: close the device. Call it
    /// from CallKit's `provider(_:didDeactivate:)`. Until the session is
    /// active again no frames are produced, and the gap shows in `ts48k` as
    /// for [`AudioEngine::suspend`].
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub fn session_deactivated(&self) {
        self.phone(PhoneCmd::Session(false));
    }

    /// The OS's audio route changed (on iOS, from
    /// `AVAudioSession.routeChangeNotification`). If the device rate changed
    /// (Bluetooth hands-free runs at 16 kHz), the device reopens at the new
    /// rate without ending the call; the pipeline stays at 48 kHz. On Android
    /// the streams are always reopened on the new route.
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    pub fn route_changed(&self) {
        self.phone(PhoneCmd::RouteChanged);
    }

    /// The latest levels (updated about every 50 ms). [`Event::Levels`]
    /// delivers the same readings as they happen.
    pub fn levels(&self) -> Levels {
        lock(&self.shared.snapshot).levels.clone()
    }

    /// The latest statistics (updated with the levels).
    pub fn stats(&self) -> Stats {
        let mut s = lock(&self.shared.snapshot).stats.clone();
        let io = &self.shared.io;
        s.capture.capture_overruns = io.capture_overruns.load(Ordering::Relaxed);
        s.capture.playout_underruns = io.playout_underruns.load(Ordering::Relaxed);
        s.recv_queue_drops = self.shared.recv_drops.load(Ordering::Relaxed);
        s
    }

    /// The available devices (kept current by hot-plug polling).
    pub fn devices(&self) -> DeviceList {
        lock(&self.shared.devices).clone()
    }

    /// The devices in use.
    pub fn devices_in_use(&self) -> DevicesInUse {
        lock(&self.shared.in_use).clone()
    }

    /// Switch devices. Waits (up to 10 s) for the new device to open.
    ///
    /// An input switch returns [`crate::ErrorCode::NoPermission`] if the
    /// microphone is refused. While the OS prompt is still up it returns
    /// `Ok` and the chosen device opens if the person says yes; see
    /// [`AudioEngine::start`].
    pub fn set_device(&self, sel: DeviceSelection) -> Result<(), AudioError> {
        let (tx, rx) = sync_channel(1);
        if self
            .shared
            .dev_req
            .push(DevRequest { sel, reply: tx })
            .is_err()
        {
            return Err(AudioError::new(
                ErrorCode::DeviceBusy,
                "too many device requests",
            ));
        }
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(r) => r,
            Err(_) => Err(AudioError::new(
                ErrorCode::Backend,
                "device switch timed out",
            )),
        }
    }

    /// Events: device changes, errors and levels.
    pub fn events(&self) -> EventRx {
        EventRx {
            q: self.shared.events.clone(),
        }
    }

    /// Stop the threads and close the devices. Also done on drop.
    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::Release);
        let threads: Vec<JoinHandle<()>> = lock(&self.threads).drain(..).collect();
        for t in threads {
            let _ = t.join();
        }
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn dsp_main(shared: Arc<Shared>, mut core: Core, poll: Duration) {
    let mut cap: Option<Box<CaptureIo>> = None;
    let mut play: Option<Box<PlayIo>> = None;
    let mut block = [0.0f32; BLOCK_SAMPLES];
    let mut out = [0.0f32; BLOCK_SAMPLES];
    let mut soft_clock: Option<Instant> = None;
    let mut publish = false;
    const BLOCK: Duration = Duration::from_millis(10);
    const MAX_CAPTURE_BLOCKS_PER_PASS: usize = 8;

    while !shared.stop.load(Ordering::Acquire) {
        while let Some(c) = shared.ctrl.pop() {
            match c {
                Ctrl::Capture(on) => core.set_capture(on),
                Ctrl::Deafen(on) => core.set_deafened(on),
                Ctrl::Encoder(cfg) => {
                    if let Err(e) = core.set_encoder(cfg) {
                        shared.emit(Event::Error(e));
                    }
                }
                Ctrl::Add(p) => {
                    if let Err(p) = core.add(p) {
                        let _ = shared.garbage.push(Garbage::Participant(p));
                    }
                }
                Ctrl::Remove(id) => {
                    if let Some(p) = core.remove(id) {
                        let _ = shared.garbage.push(Garbage::Participant(p));
                    }
                }
                Ctrl::CaptureIo(c) => {
                    if let Some(old) = std::mem::replace(&mut cap, c) {
                        let _ = shared.garbage.push(Garbage::Capture(old));
                    }
                }
                Ctrl::PlayIo(p) => {
                    soft_clock = None;
                    if let Some(old) = std::mem::replace(&mut play, p) {
                        let _ = shared.garbage.push(Garbage::Play(old));
                    }
                }
                Ctrl::Pause { why, on } => {
                    if let Some(f) = core.set_paused(why, on, Instant::now()) {
                        if shared.frames.push(f).is_ok() {
                            core.capture_stats.frames_sent += 1;
                        } else {
                            core.capture_stats.frames_dropped += 1;
                        }
                    }
                }
            }
        }
        while let Some(item) = shared.recv.pop() {
            match item {
                RecvItem::Frame(f) => core.insert(f),
                RecvItem::Lost { participant, seq } => core.lost(participant, seq),
            }
        }

        let now = Instant::now();
        // Playout: keep the output ring at its target, or with no output
        // device, render on the wall clock so receive still runs.
        match play.as_mut() {
            Some(p) => {
                while p.filled() < p.target_fill {
                    core.render_block(now, &mut out);
                    match p.rs.process(&out) {
                        Ok(samples) => {
                            for &v in samples {
                                if p.tx.push(v).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            None => {
                let clk = soft_clock.get_or_insert(now);
                if now.saturating_duration_since(*clk) > Duration::from_millis(200) {
                    *clk = now;
                }
                while *clk <= now {
                    core.render_block(now, &mut out);
                    *clk += BLOCK;
                }
            }
        }

        // Capture: process the complete blocks the device has delivered, up
        // to a limit, so a thread that has fallen behind still gets back to
        // the control queue. A mute can't wait for a backlog to clear.
        if let Some(c) = cap.as_mut() {
            for _ in 0..MAX_CAPTURE_BLOCKS_PER_PASS {
                let need = c.rs.input_needed();
                if c.rx.slots() < need {
                    break;
                }
                for slot in c.buf[..need].iter_mut() {
                    *slot = c.rx.pop().unwrap_or(0.0);
                }
                if c.rs.process(&c.buf[..need], &mut block).is_err() {
                    break;
                }
                if let Some(f) = core.capture_block(&block) {
                    if shared.frames.push(f).is_ok() {
                        core.capture_stats.frames_sent += 1;
                    } else {
                        core.capture_stats.frames_dropped += 1;
                    }
                }
            }
        }

        if core.take_levels_due() {
            publish = true;
        }
        if publish {
            if let Ok(mut s) = shared.snapshot.try_lock() {
                core.fill_levels(&mut s.levels);
                core.fill_stats(&mut s.stats);
                s.version += 1;
                publish = false;
            }
        }
        std::thread::sleep(poll);
    }
}

pub(crate) enum CtlBackend {
    Null,
    #[cfg(feature = "cpal")]
    System {
        input: DeviceChoice,
        output: DeviceChoice,
    },
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    Phone {
        io: Box<dyn crate::phone::PhoneIo>,
        session_active: bool,
    },
}

#[cfg(feature = "cpal")]
type SystemBackend = crate::system::System;

/// Without the `cpal` feature there is no system backend.
#[cfg(not(feature = "cpal"))]
enum SystemBackend {}

#[cfg(not(feature = "cpal"))]
impl SystemBackend {
    fn tick(&mut self, _: &Arc<Shared>) {
        match *self {}
    }
    fn set_device(&mut self, _: &Arc<Shared>, _: DeviceSelection) -> Result<(), AudioError> {
        match *self {}
    }
}

fn ctl_main(shared: Arc<Shared>, backend: CtlBackend) {
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    let mut phone: Option<crate::phone::Phone> = None;
    let mut system: Option<SystemBackend> = match backend {
        CtlBackend::Null => None,
        #[cfg(feature = "cpal")]
        CtlBackend::System { input, output } => {
            Some(crate::system::System::start(&shared, input, output))
        }
        #[cfg(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        ))]
        CtlBackend::Phone { io, session_active } => {
            phone = Some(crate::phone::Phone::start(&shared, io, session_active));
            None
        }
    };

    let mut last_levels = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        while shared.garbage.pop().is_some() {}

        while let Some(req) = shared.dev_req.pop() {
            #[cfg(any(
                test,
                all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
            ))]
            if phone.is_some() {
                let _ = req.reply.send(crate::phone::check_selection(&req.sel));
                continue;
            }
            let r = match system.as_mut() {
                Some(sys) => sys.set_device(&shared, req.sel),
                None => crate::null::check_selection(&req.sel),
            };
            let _ = req.reply.send(r);
        }

        // Other backends have nothing to do with these.
        while let Some(_cmd) = shared.phone_cmd.pop() {
            #[cfg(any(
                test,
                all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
            ))]
            if let Some(p) = phone.as_mut() {
                p.command(&shared, _cmd);
            }
        }

        if let Some(sys) = system.as_mut() {
            sys.tick(&shared);
        }
        #[cfg(any(
            test,
            all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
        ))]
        if let Some(p) = phone.as_mut() {
            p.tick(&shared);
        }

        let levels = {
            let g = lock(&shared.snapshot);
            (g.version != last_levels).then(|| {
                last_levels = g.version;
                g.levels.clone()
            })
        };
        if let Some(l) = levels {
            shared.emit(Event::Levels(l));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Close devices before the rings go away.
    #[allow(clippy::drop_non_drop)]
    drop(system);
    #[cfg(any(
        test,
        all(feature = "os-voice", any(target_os = "ios", target_os = "android"))
    ))]
    drop(phone);
    while shared.garbage.pop().is_some() {}
}

/// The default-choice helper for backends.
pub(crate) fn is_default(c: &Option<DeviceChoice>) -> bool {
    matches!(c, None | Some(DeviceChoice::Default))
}
