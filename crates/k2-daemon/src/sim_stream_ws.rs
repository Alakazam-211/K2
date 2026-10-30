//! `GET /cli/sim/stream` (prd-mobile-dev-v1 §7, S3; vs-live M-A4, M-A19):
//! the live device over a WebSocket, plus people's input. Wire format:
//! `k2_core::sim::stream`.
//!
//! **One capture per device, many viewers.** A [`Hub`] per sim session
//! runs one capture thread: it pulls the lane's [`FrameSource`] at the
//! fastest viewer's rate and publishes a [`Shared`] frame into a `watch`
//! channel, which keeps only the newest. JPEG viewers share one JPEG
//! encode per frame (done on the capture thread while any JPEG viewer
//! watches). An H.264 viewer (0.46.7, negotiated in `hello`) has its own
//! encoder thread ([`video_worker`]): VideoToolbox on a Mac, nothing
//! elsewhere (the stream stays JPEG).
//!
//! **Flow.** Each viewer task sends the newest frame when its ack window
//! has room (`k2_simvideo::flow::AckWindow`: up to the negotiated `win`,
//! only as many as the round trip needs) and its fps cap allows. A viewer
//! that offers nothing gets one frame in flight, as before. Frames a viewer
//! can't take yet are skipped, never queued, and for H.264 they are
//! skipped before encoding, so the reference chain never breaks. H.264
//! keyframes come on join, on a size change (rotation) and when the viewer
//! asks (`{"t":"keyframe"}`); the bitrate follows the frame size and fps
//! and steps down while acks show a queue building.
//!
//! **Auth.** People only: the owner token or a Connect login (re-checked
//! every 5 s, like `/cli/sessions/bytes`; a revoked login is closed with
//! 4401). App passes, API keys and agent passports never upgrade (agents
//! read the tree and take screenshots instead). Input needs control: the
//! owner and Owner/Admin logins; Member logins watch only.
//!
//! **Rates.** Loopback viewers up to 30 fps (default 30); remote viewers
//! (LAN / Connect) 8 fps by default, 2–15 on request; remote H.264 viewers
//! 24 by default, 2–30.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::sim::encode;
use k2_core::sim::hold::HoldView;
use k2_core::sim::lane::{Action, Frame, FrameOpts, KeyMods, Point, TouchPhase};
use k2_core::sim::manager::{self, Ws};
use k2_core::sim::stream::*;
use k2_core::sim::video::flow::{AckWindow, RateCtl, ACK_EXPIRY};
use k2_core::sim::video::{self, EncoderConfig, VideoEncoder};

use crate::sidecar_routes::Caller;

/// One captured frame, shared by every viewer.
pub struct Shared {
    /// Per device (the K2SF / K2SV header's seq).
    pub seq: u32,
    pub frame: Frame,
    /// The screen in points for this frame's orientation.
    pub width_pt: u16,
    pub height_pt: u16,
    /// Header + JPEG, encoded once by whoever needs it first.
    jpeg: OnceLock<Result<Vec<u8>, String>>,
}

impl Shared {
    pub fn new(seq: u32, frame: Frame) -> Shared {
        // Points: the frame is points-sized (scale 1) unless the lane says
        // otherwise.
        let (wpt, hpt) =
            if (frame.scale - 1.0).abs() < 1e-3 { (frame.width_px, frame.height_px) } else { encode::points_size(&frame) };
        Shared { seq, frame, width_pt: wpt as u16, height_pt: hpt as u16, jpeg: OnceLock::new() }
    }

    /// The K2SF message (header + JPEG), encoding it on first use.
    pub fn jpeg_message(&self, encodes: &AtomicU64) -> Result<&[u8], String> {
        self.jpeg
            .get_or_init(|| {
                let (jpeg, w, h) = encode::frame_jpeg(&self.frame, encode::JPEG_QUALITY)?;
                encodes.fetch_add(1, Ordering::SeqCst);
                let header = FrameHeader { seq: self.seq, width_px: w as u16, height_px: h as u16, width_pt: self.width_pt, height_pt: self.height_pt };
                Ok(header.encode(&jpeg))
            })
            .as_deref()
            .map_err(Clone::clone)
    }

    fn jpeg_ready(&self) -> Option<Result<Vec<u8>, String>> {
        self.jpeg.get().map(|r| r.clone())
    }
}

/// What the capture thread publishes.
#[derive(Clone)]
pub enum Pub {
    Starting,
    Frame(Arc<Shared>),
    /// The device stopped or the capture failed for good.
    Ended(String),
}

/// One device's capture.
pub struct Hub {
    gen: u64,
    rx: watch::Receiver<Pub>,
    /// Highest fps any viewer asked for (the capture's pace).
    want_fps: Arc<Mutex<HashMap<u64, u32>>>,
    encodes: Arc<AtomicU64>,
    /// Viewers on the JPEG wire: while any watch, the capture thread
    /// encodes each frame's JPEG (once, for all of them).
    jpeg_viewers: Arc<AtomicUsize>,
}

fn hubs() -> &'static Mutex<HashMap<i64, Arc<Hub>>> {
    static H: OnceLock<Mutex<HashMap<i64, Arc<Hub>>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many JPEG encodes a device's hub has done (tests: one encode per
/// frame however many viewers).
pub fn encodes(sim_id: i64) -> Option<u64> {
    hubs().lock().ok()?.get(&sim_id).map(|h| h.encodes.load(Ordering::SeqCst))
}

/// Join (or start) the hub for a running device.
pub fn join(ws: &Ws) -> Result<(i64, Arc<Hub>), manager::SimError> {
    let m = manager::manager();
    let row = m.running(ws)?;
    let mut map = hubs().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = map.get(&row.id) {
        if !matches!(&*h.rx.borrow(), Pub::Ended(_)) {
            return Ok((row.id, h.clone()));
        }
    }
    let source = m.frames(&row, FrameOpts { max_fps: LOCAL_MAX_FPS, max_width_px: None })?;
    let (tx, rx) = watch::channel(Pub::Starting);
    let want_fps = Arc::new(Mutex::new(HashMap::new()));
    let encodes = Arc::new(AtomicU64::new(0));
    let jpeg_viewers = Arc::new(AtomicUsize::new(0));
    static GEN: AtomicU64 = AtomicU64::new(1);
    let gen = GEN.fetch_add(1, Ordering::SeqCst);
    let hub = Arc::new(Hub { gen, rx, want_fps: want_fps.clone(), encodes: encodes.clone(), jpeg_viewers: jpeg_viewers.clone() });
    map.insert(row.id, hub.clone());
    let sim_id = row.id;
    std::thread::Builder::new()
        .name(format!("k2-sim-encode-{sim_id}"))
        .spawn(move || capture(sim_id, gen, source, tx, want_fps, encodes, jpeg_viewers))
        .map_err(|e| manager::SimError::new(500, "sim_io", format!("encoder thread: {e}")))?;
    Ok((sim_id, hub))
}

fn capture(
    sim_id: i64,
    gen: u64,
    mut source: Box<dyn k2_core::sim::lane::FrameSource>,
    tx: watch::Sender<Pub>,
    want_fps: Arc<Mutex<HashMap<u64, u32>>>,
    encodes: Arc<AtomicU64>,
    jpeg_viewers: Arc<AtomicUsize>,
) {
    let mut seq: u32 = 0;
    let born = Instant::now();
    let mut last = Instant::now() - Duration::from_secs(1);
    loop {
        // Only the hub's own receiver is left: nobody watches (after a
        // grace for the first viewer to attach).
        if tx.receiver_count() <= 1 && born.elapsed() > Duration::from_secs(5) {
            break;
        }
        let fps = want_fps.lock().map(|m| m.values().copied().max().unwrap_or(REMOTE_DEFAULT_FPS)).unwrap_or(REMOTE_DEFAULT_FPS).clamp(1, LOCAL_MAX_FPS);
        let gap = Duration::from_millis(1000 / fps as u64);
        let since = last.elapsed();
        if since < gap {
            std::thread::sleep(gap - since);
        }
        match source.next_frame(Duration::from_millis(500)) {
            Ok(None) => continue, // a still screen costs nothing
            Ok(Some(f)) => {
                seq = seq.wrapping_add(1);
                let shared = Arc::new(Shared::new(seq, f));
                if jpeg_viewers.load(Ordering::SeqCst) > 0 {
                    if let Err(e) = shared.jpeg_message(&encodes) {
                        log_debug!("[sim] encode failed on s{sim_id}: {e}");
                    }
                }
                let _ = tx.send(Pub::Frame(shared));
                last = Instant::now();
            }
            Err(e) => {
                log_debug!("[sim] frame source ended on s{sim_id}: {}", e.message());
                let _ = tx.send(Pub::Ended(e.message().to_string()));
                break;
            }
        }
    }
    if let Ok(mut map) = hubs().lock() {
        if map.get(&sim_id).is_some_and(|h| h.gen == gen) {
            map.remove(&sim_id);
        }
    }
}

// ── H.264, one encoder per viewer ─────────────────────────────────────

/// One frame for a viewer's encoder.
pub struct VideoJob {
    pub shared: Arc<Shared>,
    pub force_key: bool,
    pub fps: u32,
    pub bitrate: u32,
    /// Capture time, ms since this viewer's stream began.
    pub ts_ms: u32,
}

#[derive(Debug)]
pub enum VideoOut {
    /// A K2SV message ready to send.
    Frame { seq: u32, key: bool, message: Vec<u8> },
    /// Rate control dropped it: free its slot.
    Dropped { seq: u32 },
    /// The encoder can't go on: fall back to JPEG.
    Failed { seq: u32, why: String },
}

/// Encode one job with `enc` (opened, or reopened on a size change, which
/// starts with a keyframe). Returns the K2SV message, or None when rate
/// control dropped the frame.
pub fn encode_job(enc: &mut Option<Box<dyn VideoEncoder>>, job: &VideoJob) -> Result<Option<(bool, Vec<u8>)>, String> {
    let rgb = encode::frame_rgb_points(&job.shared.frame)?;
    let (w, h) = rgb.dimensions();
    let reopen = enc.as_ref().is_none_or(|e| {
        let c = e.config();
        (c.width, c.height) != (w, h)
    });
    if reopen {
        drop(enc.take()); // free the old session first
        *enc = Some(video::open_h264(&EncoderConfig { width: w, height: h, fps: job.fps, bitrate_bps: job.bitrate })?);
    }
    let e = enc.as_mut().ok_or("no encoder")?;
    let c = e.config();
    if c.fps != job.fps || c.bitrate_bps != job.bitrate {
        e.set_rate(job.fps, job.bitrate);
    }
    let Some(chunk) = e.encode_rgb(rgb.as_raw(), job.force_key || reopen, job.ts_ms)? else {
        return Ok(None);
    };
    let age = job.shared.frame.captured_at.elapsed().as_millis().min(u16::MAX as u128) as u16;
    let header = VideoHeader {
        seq: job.shared.seq,
        width_px: w as u16,
        height_px: h as u16,
        width_pt: job.shared.width_pt,
        height_pt: job.shared.height_pt,
        codec: VIDEO_CODEC_H264,
        key: chunk.key,
        age_ms: age,
        ts_ms: job.ts_ms,
    };
    Ok(Some((chunk.key, header.encode(&chunk.data))))
}

/// The viewer's encoder thread: one job at a time (the viewer only hands
/// over a frame when the last one is out), until the viewer leaves.
pub fn video_worker(jobs: std::sync::mpsc::Receiver<VideoJob>, out: tokio::sync::mpsc::Sender<VideoOut>) {
    let mut enc: Option<Box<dyn VideoEncoder>> = None;
    while let Ok(job) = jobs.recv() {
        let seq = job.shared.seq;
        let o = match encode_job(&mut enc, &job) {
            Ok(Some((key, message))) => VideoOut::Frame { seq, key, message },
            Ok(None) => VideoOut::Dropped { seq },
            Err(why) => VideoOut::Failed { seq, why },
        };
        let failed = matches!(o, VideoOut::Failed { .. });
        if out.blocking_send(o).is_err() || failed {
            break;
        }
    }
}

struct VideoPipe {
    jobs: std::sync::mpsc::SyncSender<VideoJob>,
    out: tokio::sync::mpsc::Receiver<VideoOut>,
}

fn spawn_video_pipe(viewer: u64) -> Result<VideoPipe, String> {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::sync_channel::<VideoJob>(1);
    let (out_tx, out_rx) = tokio::sync::mpsc::channel::<VideoOut>(2);
    std::thread::Builder::new()
        .name(format!("k2-sim-h264-{viewer}"))
        .spawn(move || video_worker(jobs_rx, out_tx))
        .map_err(|e| format!("H.264 thread: {e}"))?;
    Ok(VideoPipe { jobs: jobs_tx, out: out_rx })
}

/// Who is watching (decided by the dispatcher from the credential).
#[derive(Debug, Clone)]
pub struct Viewer {
    pub caller: Caller,
    pub token: String,
    pub remote: bool,
}

fn clamp_fps(codec: Codec, remote: bool, asked: Option<u32>) -> u32 {
    match (codec, remote) {
        (_, false) => asked.unwrap_or(LOCAL_MAX_FPS).clamp(1, LOCAL_MAX_FPS),
        (Codec::Jpeg, true) => asked.unwrap_or(REMOTE_DEFAULT_FPS).clamp(REMOTE_MIN_FPS, REMOTE_MAX_FPS),
        (Codec::H264, true) => asked.unwrap_or(REMOTE_VIDEO_DEFAULT_FPS).clamp(REMOTE_MIN_FPS, REMOTE_VIDEO_MAX_FPS),
    }
}

fn who_name(c: &Caller) -> String {
    match c {
        Caller::Login { username } => username.clone(),
        _ => "owner".to_string(),
    }
}

/// The viewer message → a lane action (points).
fn to_action(m: &ViewerMsg) -> Option<Action> {
    Some(match m {
        ViewerMsg::Touch { phase, x, y, x2, y2, .. } => match (x2, y2) {
            (None, None) => Action::Touch { phase: *phase, at: Point { x: *x, y: *y } },
            // A pinch: both fingers, or the lane refuses it. Never one.
            (Some(x2), Some(y2)) => Action::TwoTouch { phase: *phase, a: Point { x: *x, y: *y }, b: Point { x: *x2, y: *y2 } },
            _ => return None,
        },
        ViewerMsg::Tap { x, y } => Action::Tap { at: Point { x: *x, y: *y } },
        ViewerMsg::Swipe { x1, y1, x2, y2, duration_ms } => Action::Swipe {
            from: Point { x: *x1, y: *y1 },
            to: Point { x: *x2, y: *y2 },
            duration: Duration::from_millis(duration_ms.unwrap_or(250).clamp(50, 3000) as u64),
        },
        // A person's keyboard (caps: `check_input`; the rate: the viewer's
        // `InputBudget`). Typing goes as key events with no read-back.
        ViewerMsg::Text { .. } | ViewerMsg::Paste { .. } if check_input(m).is_err() => return None,
        ViewerMsg::Text { text } => Action::Keystrokes { text: k2_core::sim::lane::SimText::plain(text.clone()) },
        ViewerMsg::Paste { text } => Action::Paste { text: k2_core::sim::lane::SimText::plain(text.clone()) },
        ViewerMsg::Key { key, shift, alt } => Action::Key { key: *key, mods: KeyMods { shift: *shift, alt: *alt } },
        ViewerMsg::Edit { op } => Action::Edit { op: *op },
        ViewerMsg::Button { button } => Action::Button { button: *button },
        ViewerMsg::Rotate { orientation } => Action::Rotate { orientation: *orientation },
        ViewerMsg::Appearance { appearance } => Action::Appearance { appearance: *appearance },
        ViewerMsg::OpenUrl { url } => {
            if url.len() > 2048 || url.chars().any(char::is_control) || !url.contains(':') {
                return None;
            }
            Action::OpenUrl { url: url.clone() }
        }
        _ => return None,
    })
}

async fn close(write: &mut (impl SinkExt<Message> + Unpin), code: u16, reason: &str) {
    let _ = write
        .send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: std::borrow::Cow::Owned(reason.to_string()) })))
        .await;
}

fn text(m: &ServerMsg) -> Message {
    Message::Text(serde_json::to_string(m).unwrap_or_default())
}

/// The newest frame's JPEG message: ready (the capture thread made it) or
/// encoded now off the async runtime.
async fn jpeg_for(f: &Arc<Shared>, encodes: &Arc<AtomicU64>) -> Result<Vec<u8>, String> {
    if let Some(r) = f.jpeg_ready() {
        return r;
    }
    let (f2, e2) = (f.clone(), encodes.clone());
    tokio::task::spawn_blocking(move || f2.jpeg_message(&e2).map(<[u8]>::to_vec)).await.map_err(|e| e.to_string())?
}

/// Keyframe requests closer together than this are merged.
const KEYFRAME_MIN_GAP: Duration = Duration::from_millis(250);

/// Serve one viewer. The dispatcher has refused app passes, agents and bad
/// tokens before the upgrade.
pub async fn serve(stream: &mut TcpStream, params: HashMap<String, String>, viewer: Viewer, owner_token: String) {
    let token = params.get("workspace").cloned().unwrap_or_default();
    let ws = {
        let caller = viewer.caller.clone();
        let t = token.clone();
        match tokio::task::spawn_blocking(move || crate::sim_routes::resolve_ws_for(&caller, Some(&t))).await {
            Ok(Ok(ws)) => ws,
            Ok(Err(e)) => {
                crate::routes::http::send_response(&mut *stream, "404 Not Found", "application/json", &e.body().to_string()).await;
                return;
            }
            Err(e) => {
                crate::routes::http::send_response(&mut *stream, "500 Internal Server Error", "application/json", &format!("{{\"error\":\"{e}\"}}")).await;
                return;
            }
        }
    };
    let joined = {
        let ws2 = ws.clone();
        tokio::task::spawn_blocking(move || join(&ws2)).await
    };
    let (sim_id, hub) = match joined {
        Ok(Ok(x)) => x,
        Ok(Err(e)) => {
            crate::routes::http::send_response(&mut *stream, "409 Conflict", "application/json", &e.body().to_string()).await;
            return;
        }
        Err(e) => {
            crate::routes::http::send_response(&mut *stream, "500 Internal Server Error", "application/json", &format!("{{\"error\":\"{e}\"}}")).await;
            return;
        }
    };
    // Codec + window (0.46.7). A viewer that offers nothing gets JPEG with
    // one frame in flight: the wire as it was.
    let offer = parse_offer(params.get("codecs").map(String::as_str));
    let window_max = negotiate_window(params.get("win").map(String::as_str));
    let host_h264 = offer.contains(&Codec::H264) && tokio::task::spawn_blocking(video::h264_available).await.unwrap_or(false);
    let mut codec = negotiate(&offer, host_h264);
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let me = NEXT.fetch_add(1, Ordering::SeqCst);
    let mut vpipe: Option<VideoPipe> = None;
    if codec == Codec::H264 {
        match spawn_video_pipe(me) {
            Ok(p) => vpipe = Some(p),
            Err(e) => {
                log_debug!("[sim] {e}; viewer {me} stays on JPEG");
                codec = Codec::Jpeg;
            }
        }
    }
    let ws_conn = match tokio_tungstenite::accept_async(stream).await {
        Ok(w) => w,
        Err(e) => {
            log_debug!("[sim] stream handshake failed: {e}");
            return;
        }
    };
    let (mut write, mut read) = ws_conn.split();
    let m = manager::manager();
    m.viewer_joined(sim_id);
    let mut counted_jpeg = codec == Codec::Jpeg;
    if counted_jpeg {
        hub.jpeg_viewers.fetch_add(1, Ordering::SeqCst);
    }
    let mut fps = clamp_fps(codec, viewer.remote, params.get("fps").and_then(|f| f.parse().ok()));
    if let Ok(mut w) = hub.want_fps.lock() {
        w.insert(me, fps);
    }
    let can_control = crate::sim_routes::can_control(&viewer.caller);
    let who = who_name(&viewer.caller);
    let status = {
        let ws2 = ws.clone();
        tokio::task::spawn_blocking(move || manager::manager().status(&ws2, false)).await.ok().and_then(|r| r.ok())
    };
    let view = status.and_then(|s| s.sim);
    let hello = ServerMsg::Hello {
        sim: format!("s{sim_id}"),
        lane: view.as_ref().map(|v| v.lane).unwrap_or(k2_core::sim::LaneKind::Android),
        state: view.as_ref().map(|v| v.state).unwrap_or(SimState::Running),
        profile: view.as_ref().and_then(|v| v.profile.clone()),
        fps,
        remote: viewer.remote,
        hold: m.hold_view(sim_id),
        isolation: k2_core::sim::ISOLATION_NOTE.to_string(),
        input: view.as_ref().map(|v| m.input_caps(v.lane)),
        codec,
        window: window_max,
    };
    let mut rx = hub.rx.clone();
    let mut window = AckWindow::new(window_max);
    let mut rate = RateCtl::new(video::bitrate_for(400, 900, fps, viewer.remote));
    let started = Instant::now();
    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let mut last_sent_seq: Option<u32> = None;
    let mut video_busy = false;
    let mut want_key = true;
    let mut last_key_ask = Instant::now() - Duration::from_secs(1);
    let mut last_hold = m.hold_view(sim_id);
    if write.send(text(&hello)).await.is_err() {
        if counted_jpeg {
            hub.jpeg_viewers.fetch_sub(1, Ordering::SeqCst);
        }
        m.viewer_left(sim_id);
        return;
    }
    // People's input runs in order on its own task, so a long swipe never
    // stalls this viewer's frames; refusals come back on `err_rx`.
    let (in_tx, mut in_rx) = tokio::sync::mpsc::channel::<ViewerMsg>(64);
    let (err_tx, mut err_rx) = tokio::sync::mpsc::channel::<(String, String)>(16);
    let input_task = {
        let ws = ws.clone();
        let who = who.clone();
        let caller = viewer.caller.clone();
        tokio::spawn(async move {
            while let Some(vm) = in_rx.recv().await {
                let (ws2, who2, caller2) = (ws.clone(), who.clone(), caller.clone());
                let r = tokio::task::spawn_blocking(move || person_input(&ws2, &who2, &caller2, vm)).await;
                let err = match r {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some((e.code.to_string(), e.message)),
                    Err(e) => Some(("sim_io".to_string(), e.to_string())),
                };
                if let Some(e) = err {
                    let _ = err_tx.send(e).await;
                }
            }
        })
    };
    let mut budget = InputBudget::new(Instant::now());
    let mut auth = tokio::time::interval(Duration::from_secs(5));
    auth.tick().await;
    let mut pending = true; // send the newest frame as soon as we may
    'serve: loop {
        let gap = Duration::from_millis(1000 / fps as u64);
        if window.expire(Instant::now(), ACK_EXPIRY) > 0 {
            log_debug!("[sim] viewer {me}: acks lost for {ACK_EXPIRY:?}; freeing the window");
        }
        // Send the newest frame when allowed (window, fps cap, encoder free).
        if pending && !video_busy && window.can_send(fps) && last_sent.elapsed() >= gap {
            let p = rx.borrow_and_update().clone();
            match p {
                Pub::Frame(f) => {
                    pending = false;
                    match codec {
                        Codec::Jpeg => match jpeg_for(&f, &hub.encodes).await {
                            Ok(msg) => {
                                if write.send(Message::Binary(msg)).await.is_err() {
                                    break 'serve;
                                }
                                window.sent(f.seq, Instant::now());
                                last_sent = Instant::now();
                                last_sent_seq = Some(f.seq);
                            }
                            Err(e) => log_debug!("[sim] encode failed on s{sim_id}: {e}"),
                        },
                        Codec::H264 => {
                            // Same frame again only for a keyframe ask (a
                            // still screen sends nothing new).
                            if last_sent_seq == Some(f.seq) && !want_key {
                                continue 'serve;
                            }
                            window.cancel(f.seq);
                            let target = video::bitrate_for(f.width_pt as u32, f.height_pt as u32, fps, viewer.remote);
                            if target != rate.target() {
                                rate.retarget(target);
                            }
                            let ts_ms = f.frame.captured_at.saturating_duration_since(started).as_millis().min(u32::MAX as u128) as u32;
                            let job = VideoJob { shared: f.clone(), force_key: want_key, fps, bitrate: rate.current(), ts_ms };
                            let sent = vpipe.as_ref().is_some_and(|p| p.jobs.try_send(job).is_ok());
                            if sent {
                                want_key = false;
                                video_busy = true;
                                window.sent(f.seq, Instant::now());
                                last_sent = Instant::now();
                                last_sent_seq = Some(f.seq);
                            } else {
                                // The encoder thread is gone.
                                fall_back_to_jpeg(&mut codec, &mut vpipe, &mut counted_jpeg, &hub, &mut fps, viewer.remote, me);
                                pending = true;
                                let msg = ServerMsg::Codec { codec, message: Some("The video encoder stopped; this stream is JPEG now.".into()) };
                                if write.send(text(&msg)).await.is_err() || write.send(text(&ServerMsg::Fps { fps })).await.is_err() {
                                    break 'serve;
                                }
                            }
                        }
                    }
                }
                Pub::Ended(why) => {
                    let _ = write.send(text(&ServerMsg::State { state: SimState::Stopped, message: Some(why) })).await;
                    close(&mut write, CLOSE_DEVICE_STOPPED, "device stopped").await;
                    break 'serve;
                }
                Pub::Starting => pending = false,
            }
        }
        let video_out = async {
            match vpipe.as_mut() {
                Some(p) => p.out.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = auth.tick() => {
                if !crate::routes::http::token_still_valid(&viewer.token, &owner_token) {
                    log_debug!("[sim] stream token revoked; closing s{sim_id}");
                    close(&mut write, CLOSE_AUTH_REVOKED, "token revoked").await;
                    break 'serve;
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    close(&mut write, CLOSE_DEVICE_STOPPED, "device stopped").await;
                    break 'serve;
                }
                pending = true;
            }
            out = video_out => {
                video_busy = false;
                match out {
                    Some(VideoOut::Frame { message, .. }) => {
                        if write.send(Message::Binary(message)).await.is_err() {
                            break 'serve;
                        }
                    }
                    Some(VideoOut::Dropped { seq }) => {
                        window.cancel(seq);
                        last_sent_seq = None;
                        pending = true;
                    }
                    failed => {
                        let why = match failed {
                            Some(VideoOut::Failed { seq, why }) => {
                                window.cancel(seq);
                                why
                            }
                            _ => "the encoder thread ended".to_string(),
                        };
                        log_debug!("[sim] H.264 for viewer {me} on s{sim_id} failed ({why}); falling back to JPEG");
                        fall_back_to_jpeg(&mut codec, &mut vpipe, &mut counted_jpeg, &hub, &mut fps, viewer.remote, me);
                        last_sent_seq = None;
                        pending = true;
                        let msg = ServerMsg::Codec { codec, message: Some(format!("The video encoder stopped ({why}); this stream is JPEG now.")) };
                        if write.send(text(&msg)).await.is_err() || write.send(text(&ServerMsg::Fps { fps })).await.is_err() {
                            break 'serve;
                        }
                    }
                }
            }
            _ = tokio::time::sleep(if pending && !video_busy && window.can_send(fps) {
                gap.saturating_sub(last_sent.elapsed()).max(Duration::from_millis(2))
            } else {
                Duration::from_millis(250)
            }) => {
                // Hold changes (a tap from another viewer, take-over expiry).
                let h = m.hold_view(sim_id);
                let changed_kind = std::mem::discriminant(&h) != std::mem::discriminant(&last_hold)
                    || matches!((&h, &last_hold), (HoldView::Takeover { by: a, .. }, HoldView::Takeover { by: b, .. }) if a != b);
                if changed_kind {
                    last_hold = h.clone();
                    if write.send(text(&ServerMsg::Hold { hold: h })).await.is_err() {
                        break 'serve;
                    }
                }
            }
            Some((code, message)) = err_rx.recv() => {
                if write.send(text(&ServerMsg::Error { code, message })).await.is_err() {
                    break 'serve;
                }
            }
            msg = read.next() => {
                let Some(Ok(msg)) = msg else { break 'serve };
                let Message::Text(t) = msg else {
                    if matches!(msg, Message::Close(_)) { break 'serve; }
                    continue 'serve;
                };
                let Ok(vm) = serde_json::from_str::<ViewerMsg>(&t) else {
                    let _ = write.send(text(&ServerMsg::Error { code: "bad_message".into(), message: "unknown message".into() })).await;
                    continue 'serve;
                };
                match vm {
                    ViewerMsg::Ack { seq } => {
                        let now = Instant::now();
                        window.ack(seq, now);
                        if codec == Codec::H264 {
                            // The next job carries the new rate.
                            let _ = rate.update(window.congested(), now);
                        }
                    }
                    ViewerMsg::Fps { fps: f } => {
                        fps = clamp_fps(codec, viewer.remote, Some(f));
                        if let Ok(mut w) = hub.want_fps.lock() { w.insert(me, fps); }
                        let _ = write.send(text(&ServerMsg::Fps { fps })).await;
                    }
                    ViewerMsg::Keyframe => {
                        if codec == Codec::H264 && last_key_ask.elapsed() >= KEYFRAME_MIN_GAP {
                            last_key_ask = Instant::now();
                            want_key = true;
                            pending = true;
                        }
                    }
                    other => {
                        if !can_control {
                            let _ = write.send(text(&ServerMsg::Error { code: "sim_watch_only".into(), message: "You can watch this device but not use it.".into() })).await;
                            continue 'serve;
                        }
                        if let Err(r) = check_input(&other).and_then(|_| budget.take(&other, Instant::now())) {
                            let _ = write.send(text(&ServerMsg::Error { code: r.code.into(), message: r.message })).await;
                            continue 'serve;
                        }
                        if in_tx.try_send(other).is_err() {
                            let _ = write.send(text(&ServerMsg::Error { code: "sim_busy".into(), message: "Too much input at once; slow down.".into() })).await;
                        }
                    }
                }
            }
        }
    }
    drop(in_tx);
    input_task.abort();
    drop(vpipe);
    if counted_jpeg {
        hub.jpeg_viewers.fetch_sub(1, Ordering::SeqCst);
    }
    if let Ok(mut w) = hub.want_fps.lock() {
        w.remove(&me);
    }
    drop(rx);
    m.viewer_left(sim_id);
}

/// Switch a viewer from H.264 to JPEG on the same socket.
fn fall_back_to_jpeg(
    codec: &mut Codec,
    vpipe: &mut Option<VideoPipe>,
    counted_jpeg: &mut bool,
    hub: &Hub,
    fps: &mut u32,
    remote: bool,
    me: u64,
) {
    *codec = Codec::Jpeg;
    *vpipe = None;
    if !*counted_jpeg {
        *counted_jpeg = true;
        hub.jpeg_viewers.fetch_add(1, Ordering::SeqCst);
    }
    *fps = clamp_fps(Codec::Jpeg, remote, Some(*fps));
    if let Ok(mut w) = hub.want_fps.lock() {
        w.insert(me, *fps);
    }
}

/// One person input from the stream (blocking).
fn person_input(ws: &Ws, who: &str, caller: &Caller, vm: ViewerMsg) -> Result<(), manager::SimError> {
    let m = manager::manager();
    let actor = manager::Actor::Person {
        name: match caller {
            Caller::Login { username } => format!("login:{username}"),
            _ => "owner".into(),
        },
    };
    match vm {
        ViewerMsg::TakeOver => m.hold(ws, &actor, k2_core::sim::api::HoldAction::TakeOver).map(|_| ()),
        ViewerMsg::HandBack => m.hold(ws, &actor, k2_core::sim::api::HoldAction::HandBack).map(|_| ()),
        // One finger lifting with a velocity: the daemon replays the
        // flick's tail on the device's clock first.
        ViewerMsg::Touch { phase: TouchPhase::Up, x, y, x2: None, y2: None, vx: Some(vx), vy: Some(vy) } => {
            m.person_fling(ws, who, Point { x, y }, (vx, vy))
        }
        o => match to_action(&o) {
            Some(a) => m.person_action(ws, who, &a),
            None => Err(manager::SimError::new(400, "sim_invalid", "bad input")),
        },
    }
}

#[cfg(test)]
#[path = "sim_stream_ws_tests.rs"]
mod tests;
