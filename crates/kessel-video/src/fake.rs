//! A scripted backend for tests on any machine: cameras and screens that
//! draw a readable frame code, scripted permissions, hot-plug and events,
//! a structural H.264 codec, and faults.
//!
//! **The frame code.** Every fake frame carries a 32-bit counter drawn as an
//! 8 x 4 grid of black and white blocks in the top-left quarter of the
//! frame (each block 1/16 of the frame's width and height, so 80 x 45 px at
//! 1280 x 720 and 16 x 16 at 256 x 144). Bit 31 is the top-left block,
//! row by row. [`read_frame_code`] reads it back after scaling and lossy
//! coding.
//!
//! **The structural codec** ([`FakeCodec::Structural`]) writes H.264 that is
//! well formed but not decodable by a real decoder: each access unit is
//! SPS + PPS (keyframes only) + one slice NAL holding the frame code, all
//! with 4-byte start codes, sized to the target bitrate. It follows every
//! bitstream rule of a real H.264 encoder here, so packetisers, encryption
//! and RTP tests run without an OS codec. Its decoder returns a grey frame
//! with the code drawn on it.

use crate::api::{Done, EventHub, EventRx, FrameSource, VideoBackend, VideoDecoder, VideoEncoder, Wake};
use crate::convert::luma_plane;
use crate::error::{unsupported, DecodeError, VideoError};
use crate::frame::{PixelFormat, Planes, VideoFrame};
use crate::h264::{self, NAL_IDR, NAL_PPS, NAL_SLICE, NAL_SPS};
use crate::kit::{lock, now_us, CoreFrame, DecoderCore, EncoderCore, QueuedSource, SourceControl, SourceShared, ThreadedDecoder, ThreadedEncoder};
use crate::types::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A scripted permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FakePermission {
    /// Always granted.
    Granted,
    /// Always denied.
    Denied,
    /// Restricted by policy.
    Restricted,
    /// `NotDetermined` until asked; then `answer` after `after`.
    Prompt {
        /// The person's answer.
        answer: Permission,
        /// How long they take.
        after: Duration,
    },
}

/// Which codecs the fake has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FakeCodec {
    /// None.
    None,
    /// Structural H.264 (see the module docs).
    Structural,
}

/// A scripted encoder fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EncoderFault {
    /// That `submit` answers `Busy`.
    Busy,
    /// The session dies at that frame: `poll` gives a terminal error.
    SessionLost,
}

/// What the picker answers.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FakePick {
    /// The person cancels: `Err(NotFound)`.
    Cancel,
    /// The person picks this listed source.
    Choose(String),
}

/// The script.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FakeConfig {
    /// Cameras at the start.
    pub cameras: Vec<CameraInfo>,
    /// Displays and windows.
    pub screens: Vec<ScreenSourceInfo>,
    /// Hot-plug: after each delay (from the backend's start), the camera
    /// list becomes this.
    pub hotplug: Vec<(Duration, Vec<CameraInfo>)>,
    /// The camera permission.
    pub camera_permission: FakePermission,
    /// The screen-recording permission.
    pub screen_permission: FakePermission,
    /// Events every opened source sends, each after its delay from open.
    pub source_events: Vec<(Duration, SourceEvent)>,
    /// Machine-wide events, each after its delay from the backend's start.
    pub backend_events: Vec<(Duration, BackendEvent)>,
    /// The codecs.
    pub codec: FakeCodec,
    /// How many encoders may be open at once (`NoResources` beyond).
    pub max_instances: Option<u32>,
    /// Faults by submit number (1 = the first submit), for every encoder.
    pub encoder_faults: Vec<(u64, EncoderFault)>,
    /// What `pick_screen` answers.
    pub pick: FakePick,
}

impl Default for FakeConfig {
    /// One front camera (1280x720 and 640x480 at 30 fps), one display and
    /// one window, everything granted, the structural codec, no faults.
    fn default() -> Self {
        let mut cam = CameraInfo::new("fake:camera:0", "Fake Camera", Facing::Front, true);
        cam.modes = vec![
            CaptureMode::new(1280, 720, 30, PixelFormat::I420),
            CaptureMode::new(640, 480, 30, PixelFormat::I420),
        ];
        let mut window = ScreenSourceInfo::new("fake:window:1", ScreenKind::Window, "Fake Window", 800, 600);
        window.app = Some("Fake App".into());
        FakeConfig {
            cameras: vec![cam],
            screens: vec![ScreenSourceInfo::new("fake:display:0", ScreenKind::Display, "Fake Display", 1920, 1080), window],
            hotplug: Vec::new(),
            camera_permission: FakePermission::Granted,
            screen_permission: FakePermission::Granted,
            source_events: Vec::new(),
            backend_events: Vec::new(),
            codec: FakeCodec::Structural,
            max_instances: None,
            encoder_faults: Vec::new(),
            pick: FakePick::Choose("fake:display:0".into()),
        }
    }
}

struct PermSlot {
    script: FakePermission,
    now: Permission,
}

impl PermSlot {
    fn new(script: FakePermission) -> PermSlot {
        let now = match script {
            FakePermission::Granted => Permission::Granted,
            FakePermission::Denied => Permission::Denied,
            FakePermission::Restricted => Permission::Restricted,
            FakePermission::Prompt { .. } => Permission::NotDetermined,
        };
        PermSlot { script, now }
    }
}

struct Inner {
    cfg: FakeConfig,
    cameras: Mutex<Vec<CameraInfo>>,
    camera_perm: Mutex<PermSlot>,
    screen_perm: Mutex<PermSlot>,
    events: EventHub,
    shut: AtomicBool,
    encoders: Arc<AtomicU32>,
}

/// The scripted backend.
pub struct FakeBackend {
    inner: Arc<Inner>,
    script: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for FakeBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBackend").finish_non_exhaustive()
    }
}

impl FakeBackend {
    /// A backend following `cfg`. Its script clock starts now.
    pub fn new(cfg: FakeConfig) -> Arc<FakeBackend> {
        let inner = Arc::new(Inner {
            cameras: Mutex::new(cfg.cameras.clone()),
            camera_perm: Mutex::new(PermSlot::new(cfg.camera_permission)),
            screen_perm: Mutex::new(PermSlot::new(cfg.screen_permission)),
            events: EventHub::default(),
            shut: AtomicBool::new(false),
            encoders: Arc::new(AtomicU32::new(0)),
            cfg,
        });
        let script = if inner.cfg.hotplug.is_empty() && inner.cfg.backend_events.is_empty() {
            None
        } else {
            let i = inner.clone();
            Some(std::thread::spawn(move || run_script(i)))
        };
        Arc::new(FakeBackend {
            inner,
            script: Mutex::new(script),
        })
    }

    fn live(&self) -> Result<(), VideoError> {
        if self.inner.shut.load(Ordering::Acquire) {
            Err(VideoError::ShutDown)
        } else {
            Ok(())
        }
    }

    fn request(&self, slot: &Mutex<PermSlot>, done: Done<Permission>, which: fn(&Inner) -> &Mutex<PermSlot>) {
        let (now, script) = {
            let s = lock(slot);
            (s.now, s.script)
        };
        match (now, script) {
            (Permission::NotDetermined, FakePermission::Prompt { answer, after }) => {
                let inner = self.inner.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(after);
                    lock(which(&inner)).now = answer;
                    done(answer);
                });
            }
            (p, _) => done(p),
        }
    }
}

impl Drop for FakeBackend {
    fn drop(&mut self) {
        self.inner.shut.store(true, Ordering::Release);
        if let Some(t) = lock(&self.script).take() {
            let _ = t.join();
        }
    }
}

fn run_script(inner: Arc<Inner>) {
    let start = Instant::now();
    let mut steps: Vec<(Duration, Option<Vec<CameraInfo>>, Option<BackendEvent>)> = Vec::new();
    for (d, cams) in &inner.cfg.hotplug {
        steps.push((*d, Some(cams.clone()), None));
    }
    for (d, e) in &inner.cfg.backend_events {
        steps.push((*d, None, Some(e.clone())));
    }
    steps.sort_by_key(|s| s.0);
    for (at, cams, event) in steps {
        while start.elapsed() < at {
            if inner.shut.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if let Some(c) = cams {
            *lock(&inner.cameras) = c.clone();
            inner.events.publish(BackendEvent::Cameras(c));
        }
        if let Some(e) = event {
            inner.events.publish(e);
        }
    }
}

impl VideoBackend for FakeBackend {
    fn capabilities(&self) -> Capabilities {
        let mut caps = Capabilities::new("fake", crate::platform::os_name());
        crate::rust_codecs::add_capabilities(&mut caps);
        caps.camera = true;
        caps.screen = ScreenCapability {
            supported: true,
            system_picker: true,
            list: true,
            windows: true,
            cursor: true,
            exclude_own_windows: true,
        };
        if self.inner.cfg.codec == FakeCodec::Structural {
            for dir in [CodecDirection::Encode, CodecDirection::Decode] {
                let mut c = CodecCapability::new(VideoCodec::H264, dir, Implementation::Software, "fake-structural", 3840, 2160, 60);
                c.max_instances = self.inner.cfg.max_instances;
                c.h264 = Some(H264Capability::new(
                    vec![H264Profile::ConstrainedBaseline, H264Profile::ConstrainedHigh],
                    H264Level::L5_1,
                ));
                c.formats = vec![PixelFormat::I420, PixelFormat::Nv12, PixelFormat::Bgra, PixelFormat::Rgba];
                caps.codecs.push(c);
            }
        }
        caps
    }

    fn now_us(&self) -> u64 {
        now_us()
    }

    fn cameras(&self) -> Vec<CameraInfo> {
        lock(&self.inner.cameras).clone()
    }

    fn camera_permission(&self) -> Permission {
        lock(&self.inner.camera_perm).now
    }

    fn request_camera_permission(&self, done: Done<Permission>) {
        self.request(&self.inner.camera_perm, done, |i| &i.camera_perm);
    }

    fn open_camera(&self, id: Option<&str>, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        c.validate()?;
        if self.camera_permission() != Permission::Granted {
            return Err(VideoError::NoPermission);
        }
        let cams = self.cameras();
        let cam = match id {
            Some(id) => cams.iter().find(|c| c.id == id),
            None => cams.iter().find(|c| c.is_default).or(cams.first()),
        }
        .ok_or(VideoError::NotFound)?;
        let mode = nearest_mode(&cam.modes, &c).unwrap_or(CaptureMode::new(1280, 720, 30, PixelFormat::I420));
        open_source(&self.inner, mode.width, mode.height, mode.max_fps, c, wake)
    }

    fn screen_permission(&self) -> Permission {
        lock(&self.inner.screen_perm).now
    }

    fn request_screen_permission(&self, done: Done<Permission>) {
        self.request(&self.inner.screen_perm, done, |i| &i.screen_perm);
    }

    fn screen_sources(&self) -> Result<Vec<ScreenSourceInfo>, VideoError> {
        self.live()?;
        Ok(self.inner.cfg.screens.clone())
    }

    fn pick_screen(&self, _opts: PickOptions, done: Done<Result<ScreenSelection, VideoError>>) {
        if self.live().is_err() {
            return done(Err(VideoError::ShutDown));
        }
        match &self.inner.cfg.pick {
            FakePick::Cancel => done(Err(VideoError::NotFound)),
            FakePick::Choose(id) => done(Ok(ScreenSelection::Picked(Arc::new(id.clone())))),
        }
    }

    fn open_screen(&self, source: &ScreenSelection, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
        self.live()?;
        c.validate()?;
        if self.screen_permission() != Permission::Granted {
            return Err(VideoError::NoPermission);
        }
        let id = match source {
            ScreenSelection::Listed(id) => id.clone(),
            ScreenSelection::Picked(any) => any.downcast_ref::<String>().cloned().ok_or(VideoError::NotFound)?,
        };
        let s = self.inner.cfg.screens.iter().find(|s| s.id == id).ok_or(VideoError::NotFound)?;
        let fps = c.fps.unwrap_or(30);
        open_source(&self.inner, s.width & !1, s.height & !1, fps, c, wake)
    }

    fn encoder(&self, config: EncoderConfig, wake: Wake) -> Result<Box<dyn VideoEncoder>, VideoError> {
        self.live()?;
        if let Some(r) = crate::rust_codecs::encoder(&config, &wake) {
            return r;
        }
        config.validate()?;
        if self.inner.cfg.codec != FakeCodec::Structural || config.codec.codec() != VideoCodec::H264 {
            return Err(unsupported(format!("the fake has no {:?} encoder", config.codec.codec())));
        }
        if config.implementation == ImplPreference::HardwareOnly {
            return Err(unsupported("the fake has no hardware encoder"));
        }
        let n = self.inner.encoders.fetch_add(1, Ordering::AcqRel) + 1;
        let slot = InstanceSlot(self.inner.encoders.clone());
        if self.inner.cfg.max_instances.is_some_and(|m| n > m) {
            return Err(VideoError::NoResources);
        }
        let submits = Arc::new(AtomicU64::new(0));
        let core = StructuralEncoder {
            config,
            submits: submits.clone(),
            faults: self.inner.cfg.encoder_faults.clone(),
        };
        let inner = ThreadedEncoder::start(Box::new(core), config, wake)?;
        Ok(Box::new(FakeEncoder {
            inner,
            submits,
            faults: self.inner.cfg.encoder_faults.clone(),
            _slot: slot,
        }))
    }

    fn decoder(&self, config: DecoderConfig, wake: Wake) -> Result<Box<dyn VideoDecoder>, VideoError> {
        self.live()?;
        if let Some(r) = crate::rust_codecs::decoder(&config, &wake) {
            return r;
        }
        if self.inner.cfg.codec != FakeCodec::Structural || config.codec != VideoCodec::H264 {
            return Err(unsupported(format!("the fake has no {:?} decoder", config.codec)));
        }
        if config.implementation == ImplPreference::HardwareOnly {
            return Err(unsupported("the fake has no hardware decoder"));
        }
        Ok(Box::new(ThreadedDecoder::start(Box::new(StructuralDecoder), config, wake)?))
    }

    fn convert(&self, frame: &VideoFrame, width: u32, height: u32, format: PixelFormat) -> Result<VideoFrame, VideoError> {
        self.live()?;
        crate::convert::convert(frame, width, height, format)
    }

    fn subscribe(&self) -> EventRx {
        self.inner.events.subscribe()
    }

    fn shutdown(&self) {
        self.inner.shut.store(true, Ordering::Release);
    }
}

// ───────────────────────────── sources ─────────────────────────────

struct FakeControl {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SourceControl for FakeControl {}

impl Drop for FakeControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open_source(inner: &Arc<Inner>, width: u32, height: u32, fps: u32, c: CaptureConstraints, wake: Wake) -> Result<Box<dyn FrameSource>, VideoError> {
    let shared = SourceShared::new(c, fps, wake);
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st, events, backend) = (shared.clone(), stop.clone(), inner.cfg.source_events.clone(), inner.clone());
    let thread = std::thread::Builder::new()
        .name("kessel-video-fake-source".into())
        .spawn(move || {
            let start = Instant::now();
            let mut events = events;
            events.sort_by_key(|e| e.0);
            let mut events = events.into_iter().peekable();
            let period = Duration::from_micros(1_000_000 / fps.max(1) as u64);
            let mut n: u32 = 0;
            let mut next = Instant::now();
            loop {
                if st.load(Ordering::Acquire) || backend.shut.load(Ordering::Acquire) {
                    if backend.shut.load(Ordering::Acquire) {
                        s.event(SourceEvent::Ended(EndReason::Closed));
                    }
                    return;
                }
                while let Some((at, _)) = events.peek() {
                    if start.elapsed() < *at {
                        break;
                    }
                    let (_, e) = events.next().expect("peeked");
                    let ended = matches!(e, SourceEvent::Ended(_));
                    s.event(e);
                    if ended {
                        return;
                    }
                }
                if Instant::now() >= next {
                    next += period;
                    if !s.idle() {
                        match pattern_frame(width, height, n, now_us()) {
                            Ok(f) => s.deliver(f),
                            Err(e) => s.event(SourceEvent::Error(e)),
                        }
                    }
                    n = n.wrapping_add(1);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        })
        .map_err(|e| VideoError::os(e.raw_os_error().unwrap_or(0) as i64, format!("spawn: {e}")))?;
    Ok(Box::new(QueuedSource::new(
        shared,
        FakeControl {
            stop,
            thread: Some(thread),
        },
    )))
}

// ───────────────────────────── frame code ─────────────────────────────

fn block_rect(width: u32, height: u32, bit: u32) -> (usize, usize, usize, usize) {
    let (bw, bh) = ((width as usize / 16).max(1), (height as usize / 16).max(1));
    let (col, row) = ((bit % 8) as usize, (bit / 8) as usize);
    (col * bw, row * bh, bw, bh)
}

fn draw_code(y_plane: &mut [u8], stride: usize, width: u32, height: u32, code: u32) {
    for bit in 0..32 {
        let on = (code >> (31 - bit)) & 1 == 1;
        let (x0, y0, bw, bh) = block_rect(width, height, bit);
        for y in y0..(y0 + bh).min(height as usize) {
            for x in x0..(x0 + bw).min(width as usize) {
                y_plane[y * stride + x] = if on { 235 } else { 16 };
            }
        }
    }
}

/// An I420 test frame: a gradient, a bar that moves with `n`, and `n` as
/// the frame code.
pub fn pattern_frame(width: u32, height: u32, n: u32, capture_us: u64) -> Result<VideoFrame, VideoError> {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut buf = vec![0u8; w * h + 2 * cw * ch];
    let bar_w = (w / 16).max(2);
    let bar_x = (n as usize * (w / 64).max(1)) % w;
    for y in 0..h {
        for x in 0..w {
            let in_bar = x >= bar_x && x < bar_x + bar_w && y > h / 4;
            buf[y * w + x] = if in_bar { 200 } else { (40 + (x * 100 / w.max(1)) + (y * 60 / h.max(1))) as u8 };
        }
    }
    draw_code(&mut buf[..w * h], w, width, height, n);
    for i in 0..cw * ch {
        buf[w * h + i] = 110 + ((i % cw) * 30 / cw.max(1)) as u8;
        buf[w * h + cw * ch + i] = 140;
    }
    VideoFrame::from_planes(Planes::tight(PixelFormat::I420, width, height, buf)?, width, height, capture_us)
}

/// Read the frame code back (`None` if a block is neither clearly black nor
/// clearly white).
pub fn read_frame_code(frame: &VideoFrame) -> Option<u32> {
    let (y, stride) = luma_plane(frame).ok()?;
    let mut code = 0u32;
    for bit in 0..32 {
        let (x0, y0, bw, bh) = block_rect(frame.width, frame.height, bit);
        // The middle half of the block, away from blurred edges.
        let (mx0, my0) = (x0 + bw / 4, y0 + bh / 4);
        let (mx1, my1) = ((x0 + bw * 3 / 4).max(mx0 + 1), (y0 + bh * 3 / 4).max(my0 + 1));
        let mut sum = 0u64;
        let mut n = 0u64;
        for yy in my0..my1.min(frame.height as usize) {
            for xx in mx0..mx1.min(frame.width as usize) {
                sum += y[yy * stride + xx] as u64;
                n += 1;
            }
        }
        let avg = sum.checked_div(n)?;
        let bit_on = match avg {
            0..=90 => false,
            160..=255 => true,
            _ => return None,
        };
        code |= (bit_on as u32) << (31 - bit);
    }
    Some(code)
}

// ───────────────────────────── structural codec ─────────────────────────────

const MAGIC: &[u8; 4] = b"KVFK";

struct InstanceSlot(Arc<AtomicU32>);

impl Drop for InstanceSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct FakeEncoder {
    inner: ThreadedEncoder,
    submits: Arc<AtomicU64>,
    faults: Vec<(u64, EncoderFault)>,
    _slot: InstanceSlot,
}

impl VideoEncoder for FakeEncoder {
    fn submit(&mut self, frame: &VideoFrame, opts: EncodeOptions) -> Result<(), VideoError> {
        let n = self.submits.fetch_add(1, Ordering::AcqRel) + 1;
        if self.faults.contains(&(n, EncoderFault::Busy)) {
            return Err(VideoError::Busy);
        }
        self.inner.submit(frame, opts)
    }
    fn poll(&mut self) -> Option<Result<EncodedFrame, VideoError>> {
        self.inner.poll()
    }
    fn request_keyframe(&mut self) {
        self.inner.request_keyframe()
    }
    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        self.inner.set_rates(rates)
    }
    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        self.inner.set_size(width, height)
    }
    fn info(&self) -> EncoderInfo {
        self.inner.info()
    }
    fn stats(&self) -> EncoderStats {
        self.inner.stats()
    }
}

struct StructuralEncoder {
    config: EncoderConfig,
    submits: Arc<AtomicU64>,
    faults: Vec<(u64, EncoderFault)>,
}

impl EncoderCore for StructuralEncoder {
    fn input_format(&self) -> PixelFormat {
        PixelFormat::I420
    }

    fn encode(&mut self, frame: &VideoFrame, keyframe: bool) -> Result<Option<CoreFrame>, VideoError> {
        if self.faults.contains(&(self.submits.load(Ordering::Acquire), EncoderFault::SessionLost)) {
            return Err(VideoError::os(-1, "the fake session was lost (scripted)"));
        }
        let code = read_frame_code(frame).unwrap_or(0);
        let level = match self.config.codec {
            CodecConfig::H264 { level, .. } => level.0,
            _ => 31,
        };
        let (profile_idc, constraints) = match self.config.codec {
            CodecConfig::H264 { profile: H264Profile::ConstrainedHigh, .. } => (100u8, 0x0cu8),
            _ => (66, 0xe0),
        };
        let mut au = Vec::new();
        if keyframe {
            h264::push_annexb(&mut au, &[0x67, profile_idc, constraints, level, 0xda, 0x01, 0x40, 0x16, 0xe8, 0x40]);
            h264::push_annexb(&mut au, &[0x68, 0xce, 0x3c, 0x80]);
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(MAGIC);
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(&(frame.width as u16).to_be_bytes());
        payload.extend_from_slice(&(frame.height as u16).to_be_bytes());
        // Size the frame to the bitrate (keyframes 3x), so rate tests see it.
        let per_frame = (self.config.bitrate_bps / 8 / self.config.fps.max(1)) as usize;
        let target = if keyframe { per_frame * 3 } else { per_frame };
        let mut x: u32 = code ^ 0x9e37_79b9;
        while payload.len() < target.saturating_sub(16) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            payload.push(x as u8);
        }
        let mut slice = vec![if keyframe { 0x65 } else { 0x41 }];
        h264::escape_rbsp(&payload, &mut slice);
        slice.push(0x80);
        h264::push_annexb(&mut au, &slice);
        Ok(Some(CoreFrame::new(au, keyframe)))
    }

    fn set_rates(&mut self, rates: Rates) -> Result<(), VideoError> {
        self.config.bitrate_bps = rates.bitrate_bps;
        self.config.fps = rates.fps;
        Ok(())
    }

    fn set_size(&mut self, width: u32, height: u32) -> Result<(), VideoError> {
        self.config.width = width;
        self.config.height = height;
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        Implementation::Software
    }

    fn name(&self) -> String {
        "fake-structural".into()
    }
}

struct StructuralDecoder;

impl DecoderCore for StructuralDecoder {
    fn decode(&mut self, input: &EncodedInput) -> Result<Option<VideoFrame>, DecodeError> {
        let nals = h264::annexb_nals(&input.data);
        let types: Vec<u8> = nals.iter().filter_map(|n| h264::nal_type(n)).collect();
        let Some(slice) = nals.iter().find(|n| matches!(h264::nal_type(n), Some(NAL_IDR | NAL_SLICE))) else {
            return if types.iter().all(|t| matches!(*t, NAL_SPS | NAL_PPS | h264::NAL_SEI | h264::NAL_AUD)) && !types.is_empty() {
                Ok(None)
            } else {
                Err(DecodeError::Corrupt(format!("no slice in NAL types {types:?}")))
            };
        };
        if h264::nal_type(slice) == Some(NAL_IDR) && !(types.contains(&NAL_SPS) && types.contains(&NAL_PPS)) {
            return Err(DecodeError::Corrupt("an IDR without SPS and PPS in band".into()));
        }
        let payload = h264::unescape_rbsp(&slice[1..]);
        if payload.len() < 12 || &payload[..4] != MAGIC {
            return Err(DecodeError::Corrupt("not a fake-structural slice".into()));
        }
        let code = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let width = u16::from_be_bytes([payload[8], payload[9]]) as u32;
        let height = u16::from_be_bytes([payload[10], payload[11]]) as u32;
        let (w, h) = (width as usize, height as usize);
        if w == 0 || h == 0 {
            return Err(DecodeError::Corrupt(format!("a {width}x{height} frame")));
        }
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let mut buf = vec![128u8; w * h + 2 * cw * ch];
        draw_code(&mut buf[..w * h], w, width, height, code);
        let planes = Planes::tight(PixelFormat::I420, width, height, buf).map_err(DecodeError::Video)?;
        Ok(Some(VideoFrame::from_planes(planes, width, height, input.capture_us).map_err(DecodeError::Video)?))
    }

    fn reset(&mut self) -> Result<(), VideoError> {
        Ok(())
    }

    fn implementation(&self) -> Implementation {
        Implementation::Software
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::no_wake;
    use crate::convert::convert;
    use crate::kit::wait_for;

    fn counting_wake() -> (Wake, Arc<AtomicU64>) {
        let n = Arc::new(AtomicU64::new(0));
        let m = n.clone();
        (Arc::new(move || {
            m.fetch_add(1, Ordering::Relaxed);
        }), n)
    }

    #[test]
    fn the_frame_code_survives_scaling_and_conversion() {
        for code in [0u32, 1, 0xdead_beef, u32::MAX, 123_456] {
            let f = pattern_frame(1280, 720, code, 0).expect("frame");
            assert_eq!(read_frame_code(&f), Some(code));
            for (w, h, fmt) in [(640, 360, PixelFormat::Nv12), (320, 180, PixelFormat::I420), (1920, 1080, PixelFormat::Rgba), (256, 144, PixelFormat::Bgra)] {
                let s = convert(&f, w, h, fmt).expect("convert");
                assert_eq!(read_frame_code(&s), Some(code), "{code:#x} at {w}x{h} {fmt:?}");
            }
        }
        let grey = VideoFrame::filled(PixelFormat::I420, 256, 144, [128, 128, 128], 0).expect("grey");
        assert_eq!(read_frame_code(&grey), None, "grey is neither black nor white");
    }

    #[test]
    fn structural_h264_follows_the_bitstream_rules_and_round_trips() {
        let b = FakeBackend::new(FakeConfig::default());
        let (wake, wakes) = counting_wake();
        let mut enc = b.encoder(EncoderConfig::h264(640, 360, 30, 600_000), wake).expect("encoder");
        let mut dec = b.decoder(DecoderConfig::new(VideoCodec::H264), no_wake()).expect("decoder");
        let mut outs = Vec::new();
        for n in 0..12u32 {
            let f = pattern_frame(1280, 720, n, 1_000 + n as u64 * 33_333).expect("frame");
            if n == 6 {
                enc.request_keyframe();
            }
            enc.submit(&f, EncodeOptions::default()).expect("submit");
            let ef = wait_for(Duration::from_secs(2), || enc.poll()).expect("an output").expect("ok");
            outs.push(ef);
        }
        assert!(wakes.load(Ordering::Relaxed) >= 12, "a wake per output");
        for (n, ef) in outs.iter().enumerate() {
            assert_eq!(ef.capture_us, 1_000 + n as u64 * 33_333, "capture_us passes through");
            assert_eq!((ef.width, ef.height), (640, 360), "scaled to the coded size");
            assert!(ef.data.starts_with(&[0, 0, 0, 1]), "4-byte start code first");
            let nals = h264::annexb_nals(&ef.data);
            let types: Vec<u8> = nals.iter().filter_map(|n| h264::nal_type(n)).collect();
            if ef.keyframe {
                assert_eq!(types, vec![NAL_SPS, NAL_PPS, NAL_IDR]);
                assert_eq!(h264::profile_level_id(nals[0]).as_deref(), Some("42e01f"));
            } else {
                assert_eq!(types, vec![NAL_SLICE]);
            }
            // Every start code is the 4-byte form.
            let starts = ef.data.windows(3).filter(|w| *w == [0, 0, 1]).count();
            let fours = ef.data.windows(4).filter(|w| *w == [0, 0, 0, 1]).count();
            assert_eq!(starts, fours, "only 4-byte start codes");
        }
        let keys: Vec<usize> = outs.iter().enumerate().filter(|(_, e)| e.keyframe).map(|(i, _)| i).collect();
        assert_eq!(keys, vec![0, 6], "first frame and the requested one");
        for (n, ef) in outs.iter().enumerate() {
            dec.submit(EncodedInput::new(VideoCodec::H264, ef.data.clone(), ef.keyframe, ef.capture_us)).expect("submit");
            let f = wait_for(Duration::from_secs(2), || dec.poll()).expect("decoded").expect("ok");
            assert_eq!(read_frame_code(&f), Some(n as u32));
            assert_eq!(f.capture_us, ef.capture_us);
        }
        let st = enc.stats();
        assert_eq!((st.frames_in, st.frames_out, st.keyframes), (12, 12, 2));
    }

    #[test]
    fn set_rates_never_forces_a_keyframe_and_sizes_follow_the_bitrate() {
        let b = FakeBackend::new(FakeConfig::default());
        let mut enc = b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("encoder");
        let mut sizes = Vec::new();
        for n in 0..6u32 {
            if n == 3 {
                enc.set_rates(Rates::new(900_000, 30)).expect("rates");
            }
            enc.submit(&pattern_frame(320, 180, n, n as u64).expect("f"), EncodeOptions::default()).expect("submit");
            let ef = wait_for(Duration::from_secs(2), || enc.poll()).expect("out").expect("ok");
            assert_eq!(ef.keyframe, n == 0, "frame {n}");
            sizes.push(ef.data.len());
        }
        assert!(sizes[4] > sizes[2] * 2, "the bitrate change took effect: {sizes:?}");
        assert_eq!(enc.info().config.bitrate_bps, 900_000);
        enc.set_size(640, 360).expect("size");
        enc.submit(&pattern_frame(320, 180, 9, 9).expect("f"), EncodeOptions::default()).expect("submit");
        let ef = wait_for(Duration::from_secs(2), || enc.poll()).expect("out").expect("ok");
        assert!(ef.keyframe && (ef.width, ef.height) == (640, 360), "a new size starts with a keyframe");
    }

    #[test]
    fn busy_instead_of_blocking_and_scripted_faults() {
        let cfg = FakeConfig {
            encoder_faults: vec![(2, EncoderFault::Busy), (4, EncoderFault::SessionLost)],
            max_instances: Some(2),
            ..FakeConfig::default()
        };
        let b = FakeBackend::new(cfg);
        let mut e1 = b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("one");
        let e2 = b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("two");
        assert_eq!(b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).err(), Some(VideoError::NoResources));
        drop(e2);
        let _e3 = b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("a slot came free");
        let f = pattern_frame(320, 180, 1, 1).expect("f");
        e1.submit(&f, EncodeOptions::default()).expect("1");
        assert!(wait_for(Duration::from_secs(2), || e1.poll()).expect("out").is_ok());
        assert_eq!(e1.submit(&f, EncodeOptions::default()), Err(VideoError::Busy), "scripted busy");
        e1.submit(&f, EncodeOptions::default()).expect("3");
        assert!(wait_for(Duration::from_secs(2), || e1.poll()).expect("out").is_ok());
        e1.submit(&f, EncodeOptions::default()).expect("4");
        let lost = wait_for(Duration::from_secs(2), || e1.poll()).expect("out");
        assert!(matches!(lost, Err(VideoError::Os { .. })), "{lost:?}");
        assert!(e1.submit(&f, EncodeOptions::default()).is_err(), "dead after a terminal error");

        // A full pipeline answers Busy without blocking.
        let other = FakeBackend::new(FakeConfig::default());
        let mut e = other.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("encoder");
        let t = Instant::now();
        let mut busy = 0;
        for n in 0..200 {
            if e.submit(&pattern_frame(320, 180, n, n as u64).expect("f"), EncodeOptions::default()) == Err(VideoError::Busy) {
                busy += 1;
            }
        }
        assert!(busy > 0, "never busy with nobody polling");
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(e.stats().busy >= busy);
        let mut outs = 0;
        while e.poll().is_some() {
            outs += 1;
        }
        assert!(outs <= 8, "the output queue holds 8: {outs}");
    }

    #[test]
    fn the_decoder_waits_for_a_keyframe_after_reset_and_errors() {
        let b = FakeBackend::new(FakeConfig::default());
        let mut enc = b.encoder(EncoderConfig::h264(320, 180, 30, 300_000), no_wake()).expect("enc");
        let mut dec = b.decoder(DecoderConfig::new(VideoCodec::H264), no_wake()).expect("dec");
        let mut frames = Vec::new();
        for n in 0..3u32 {
            enc.submit(&pattern_frame(320, 180, n, n as u64).expect("f"), EncodeOptions::default()).expect("submit");
            frames.push(wait_for(Duration::from_secs(2), || enc.poll()).expect("out").expect("ok"));
        }
        let input = |e: &EncodedFrame| EncodedInput::new(VideoCodec::H264, e.data.clone(), e.keyframe, e.capture_us);
        assert_eq!(dec.submit(input(&frames[1])), Err(DecodeError::NeedKeyframe), "a delta first");
        dec.submit(input(&frames[0])).expect("key");
        assert!(wait_for(Duration::from_secs(2), || dec.poll()).expect("out").is_ok());
        dec.reset();
        assert_eq!(dec.submit(input(&frames[2])), Err(DecodeError::NeedKeyframe), "a delta after reset");
        dec.submit(input(&frames[0])).expect("key again");
        assert!(wait_for(Duration::from_secs(2), || dec.poll()).expect("out").is_ok());
        dec.submit(EncodedInput::new(VideoCodec::H264, vec![0, 0, 0, 1, 0x41, 1, 2, 3], false, 9)).expect("garbage is queued");
        let bad = wait_for(Duration::from_secs(2), || dec.poll()).expect("out");
        assert!(matches!(bad, Err(DecodeError::Corrupt(_))), "{bad:?}");
        assert_eq!(dec.submit(input(&frames[1])), Err(DecodeError::NeedKeyframe), "a delta after an error");
        let mut fitted = b.decoder(DecoderConfig { fit: Some((160, 160)), output: PixelFormat::Rgba, ..DecoderConfig::new(VideoCodec::H264) }, no_wake()).expect("dec");
        fitted.submit(input(&frames[0])).expect("key");
        let f = wait_for(Duration::from_secs(2), || fitted.poll()).expect("out").expect("ok");
        assert_eq!((f.width, f.height, f.format()), (160, 90, PixelFormat::Rgba));
        assert_eq!(read_frame_code(&f), Some(0));
        assert!(dec.stats().errors >= 3);
    }

    #[test]
    fn sources_deliver_coded_frames_at_the_asked_size_and_pause() {
        let cfg = FakeConfig {
            source_events: vec![(Duration::from_millis(300), SourceEvent::Interrupted(Interruption::Thermal))],
            ..FakeConfig::default()
        };
        let b = FakeBackend::new(cfg);
        let (wake, wakes) = counting_wake();
        let mut src = b.open_camera(None, CaptureConstraints::new(640, 360, 30), wake).expect("camera");
        let first = wait_for(Duration::from_secs(2), || src.try_recv()).expect("a frame");
        assert_eq!((first.width, first.height), (480, 360), "the 640x480 mode, fitted into 640x360");
        assert!(read_frame_code(&first).is_some());
        let started = wait_for(Duration::from_secs(1), || src.try_event()).expect("event");
        assert_eq!(started, SourceEvent::Started { width: 480, height: 360, fps: 30 });
        let interrupted = wait_for(Duration::from_secs(2), || src.try_event()).expect("event");
        assert_eq!(interrupted, SourceEvent::Interrupted(Interruption::Thermal));
        src.set_paused(true);
        std::thread::sleep(Duration::from_millis(100));
        while src.try_recv().is_some() {}
        std::thread::sleep(Duration::from_millis(100));
        assert!(src.try_recv().is_none(), "paused");
        src.set_paused(false);
        assert!(wait_for(Duration::from_secs(1), || src.try_recv()).is_some(), "resumed");
        assert!(wakes.load(Ordering::Relaxed) > 0);
        assert!(src.stats().frames > 3);
        let mut slow = b.open_camera(Some("fake:camera:0"), CaptureConstraints::new(320, 240, 10), no_wake()).expect("camera");
        std::thread::sleep(Duration::from_millis(550));
        let st = slow.stats();
        assert!((3..=8).contains(&st.frames), "10 fps from a 30 fps mode: {st:?}");
        assert!(st.dropped >= 6);
        assert!(slow.try_recv().is_some());
        assert_eq!(b.open_camera(Some("nope"), CaptureConstraints::default(), no_wake()).err(), Some(VideoError::NotFound));
    }

    #[test]
    fn permissions_hotplug_and_events_follow_the_script() {
        let mut cfg = FakeConfig {
            camera_permission: FakePermission::Prompt { answer: Permission::Granted, after: Duration::from_millis(50) },
            screen_permission: FakePermission::Denied,
            ..FakeConfig::default()
        };
        let usb = CameraInfo::new("fake:camera:usb", "USB Camera", Facing::External, false);
        let mut both = cfg.cameras.clone();
        both.push(usb.clone());
        cfg.hotplug = vec![(Duration::from_millis(100), both.clone())];
        cfg.backend_events = vec![(Duration::from_millis(150), BackendEvent::Thermal(ThermalLevel::Serious))];
        cfg.pick = FakePick::Cancel;
        let b = FakeBackend::new(cfg);
        let events = b.subscribe();
        assert_eq!(b.camera_permission(), Permission::NotDetermined);
        assert_eq!(b.open_camera(None, CaptureConstraints::default(), no_wake()).err(), Some(VideoError::NoPermission));
        let answer = Arc::new(Mutex::new(None));
        let a = answer.clone();
        b.request_camera_permission(Box::new(move |p| *lock(&a) = Some(p)));
        assert_eq!(wait_for(Duration::from_secs(2), || *lock(&answer)), Some(Permission::Granted));
        assert_eq!(b.camera_permission(), Permission::Granted);
        assert_eq!(wait_for(Duration::from_secs(2), || events.try_recv()), Some(BackendEvent::Cameras(both.clone())));
        assert_eq!(wait_for(Duration::from_secs(2), || events.try_recv()), Some(BackendEvent::Thermal(ThermalLevel::Serious)));
        assert_eq!(b.cameras(), both);
        assert_eq!(b.open_screen(&ScreenSelection::Listed("fake:display:0".into()), CaptureConstraints::default(), no_wake()).err(), Some(VideoError::NoPermission));
        let picked = Arc::new(Mutex::new(None));
        let p = picked.clone();
        b.pick_screen(PickOptions::default(), Box::new(move |r| *lock(&p) = Some(r.err())));
        assert_eq!(*lock(&picked), Some(Some(VideoError::NotFound)));
        b.shutdown();
        assert_eq!(b.screen_sources().err(), Some(VideoError::ShutDown));
    }

    #[test]
    fn screens_open_from_the_list_or_the_picker() {
        let b = FakeBackend::new(FakeConfig::default());
        assert_eq!(b.screen_sources().expect("list").len(), 2);
        let picked = Arc::new(Mutex::new(None));
        let p = picked.clone();
        b.pick_screen(PickOptions::default(), Box::new(move |r| *lock(&p) = Some(r)));
        let sel = lock(&picked).take().expect("answered").expect("chosen");
        let mut c = CaptureConstraints::new(1280, 720, 15);
        c.cursor = true;
        let mut src = b.open_screen(&sel, c, no_wake()).expect("screen");
        let f = wait_for(Duration::from_secs(2), || src.try_recv()).expect("frame");
        assert_eq!((f.width, f.height), (1280, 720), "1920x1080 fitted into 1280x720");
        let mut w = b.open_screen(&ScreenSelection::Listed("fake:window:1".into()), CaptureConstraints::default(), no_wake()).expect("window");
        let f = wait_for(Duration::from_secs(2), || w.try_recv()).expect("frame");
        assert_eq!((f.width, f.height), (800, 600));
    }
}
