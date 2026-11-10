//! The engine's viewer loop (fix 10, S5; PRD remote desktop §3.4 E7, E8):
//! one WebSocket viewer of one source, whatever the source is. The sim
//! route (`/cli/sim/stream`) is its first user; the desktop route
//! (`/cli/desktop/stream`, D6) is the second.
//!
//! **The engine keeps** the codec and window negotiation, the rate table
//! ([`rates`], by [`SourceKind`]), the hub subscription, the ack window and
//! [`ViewerGate`] (never a delta after a gap; a viewer that skips asks for a
//! key), fps changes, keyframe asks, the JPEG fallback when a layer's
//! encoder fails, the `stats=1` readout, the input queue and its thread,
//! and the close codes (4410 when the source ends; the route's own codes
//! from its auth hook).
//!
//! **The route gives** ([`Hooks`]):
//! - [`Auth`]: the re-check on every tick ([`AUTH_TICK`]; keep, or close
//!   with a code and reason), the check on every client frame (the login
//!   floor, security review L7), and the per-message input permission
//!   (view-only is a refusal per message; frames still flow);
//! - `limits`: input caps and rate (the sim's `check_input` +
//!   `InputBudget`), on the socket task before the queue;
//! - an [`InputSink`]: each input message, in order, on the viewer's own
//!   input thread (blocking is fine there);
//! - a [`Coalesce`] rule for what backed up in the queue ([`DesktopRule`]
//!   for pointer / wheel input; the sim's in `sim_stream_ws`);
//! - the `hello` fields besides the engine's (`codec`, `window`, `fps`,
//!   `path`), and text to push at any time ([`Push`]), such as the sim's
//!   hold changes or the desktop's cursor and control messages.
//!
//! The wire is `k2_core::sim::stream`'s: K2SF for JPEG units, K2SV for
//! video (codec byte 1 = H.264, 2 = VP8), and the `fps`, `codec`, `error`
//! and `stats` messages in the sim's shapes (the desktop's §6 says "as the
//! sim's"). A route's viewer message type implements [`StreamMsg`], which
//! names the engine's own messages (`ack`, `fps`, `keyframe`) in it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::sim::stream::{negotiate_window, negotiate_with, parse_offer, Codec, FrameHeader, InputTiming, ServerMsg, Stage, StatsWindow, VideoHeader, ViewerPath};
use k2_core::sim::stream::{CLOSE_DEVICE_STOPPED, VIDEO_CODEC_H264, VIDEO_CODEC_VP8};
use k2_core::sim::video::flow::{AckWindow, ACK_EXPIRY};
use k2_core::stream::{Codec as UnitCodec, EncodedSource, EncodedUnit, SourceKey, SourceKind};

use super::{rates, HubEvent, JoinError, Took, ViewerGate, Want};

/// How often [`Auth::recheck`] runs (as `/cli/sessions/bytes` does).
pub const AUTH_TICK: Duration = Duration::from_secs(5);
/// Inputs a viewer may have queued; one more is refused with [`Hooks::busy`].
pub const INPUT_QUEUE: usize = 64;

/// What the socket gave (a client frame, an error, or its end).
pub type WsFrame = Option<Result<Message, tokio_tungstenite::tungstenite::Error>>;

/// [`Auth::may_input`]: may this viewer send this input now?
pub type InputCheck<M> = Box<dyn FnMut(&M) -> Result<(), Refusal> + Send>;
/// [`Hooks::limits`]: input caps and rate at a time.
pub type InputLimits<M> = Box<dyn FnMut(&M, Instant) -> Result<(), Refusal> + Send>;
/// [`Hooks::hello`]: the route's `hello` fields.
pub type HelloExtras = Box<dyn FnOnce(&Negotiated) -> Map<String, Value> + Send>;
/// [`Hooks::ended`]: text before the 4410 close, from the source's reason.
pub type EndedText = Box<dyn FnMut(&str) -> Option<String> + Send>;

/// A refused input, sent back as `{"t":"error","code":…,"message":…}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: String,
    pub message: String,
}

impl Refusal {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Refusal { code: code.into(), message: message.into() }
    }
}

/// An auth check's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    /// Close the socket with this code (4401 login revoked or grant gone,
    /// 4403 forbidden, …) and reason.
    Close { code: u16, reason: String },
}

/// The engine's own viewer messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineMsg {
    /// `{"t":"ack","seq":n}`: frees the ack window up to `seq`.
    Ack { seq: u32 },
    /// `{"t":"fps","fps":n}`: a new rate (clamped by the rate table).
    Fps { fps: u32 },
    /// `{"t":"keyframe"}`: the viewer's decoder lost its place.
    Keyframe,
}

/// A route's viewer → daemon message (one JSON text frame). The engine
/// takes the ones [`Self::engine`] names; every other goes through the
/// route's [`Auth::may_input`], `limits`, queue and [`InputSink`].
pub trait StreamMsg: DeserializeOwned + Send + 'static {
    fn engine(&self) -> Option<EngineMsg>;
    /// A stats probe id (the sim's `touch` with `id`): with `stats=1`, the
    /// input's daemon-side time comes back in the readout.
    fn probe(&self) -> Option<u32> {
        None
    }
}

/// The route's auth hooks.
pub struct Auth<M> {
    /// How often [`Self::recheck`] runs ([`AUTH_TICK`]; tests go shorter).
    pub every: Duration,
    /// The re-check: the login, role and grant still allow this socket.
    /// Runs on the socket task; keep it quick (a DB read, no network).
    pub recheck: Box<dyn FnMut() -> Verdict + Send>,
    /// Every client frame, before it is read (security review L7: a login
    /// moved below the socket's floor is closed at once, not on the tick).
    pub frame: Box<dyn FnMut(&WsFrame) -> Verdict + Send>,
    /// Each input message: may this viewer send it now? (View-only is a
    /// refusal here, per message.)
    pub may_input: InputCheck<M>,
}

/// Where a viewer's input goes: one call per message, in order, on the
/// viewer's input thread, after the [`Coalesce`] rule.
pub trait InputSink<M>: Send + 'static {
    fn send(&mut self, msg: M) -> Result<(), Refusal>;
}

/// What backed up in a viewer's input queue, drained in one go: what to
/// send, in order, each with the index of the last queued message it
/// stands for (its stats probe).
pub trait Coalesce<M>: Send + 'static {
    fn coalesce(&self, batch: Vec<M>) -> Vec<(usize, M)>;
}

/// No coalescing: every message goes.
pub struct KeepAll;

impl<M: Send + 'static> Coalesce<M> for KeepAll {
    fn coalesce(&self, batch: Vec<M>) -> Vec<(usize, M)> {
        batch.into_iter().enumerate().collect()
    }
}

/// A wheel step ([`PointerWheel`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Wheel {
    pub dx: f64,
    pub dy: f64,
    /// Lines, not pixels (`mode: "line"`): only like steps sum.
    pub lines: bool,
}

/// What [`DesktopRule`] needs to see in a message.
pub trait PointerWheel {
    /// An absolute pointer move (`pointer`).
    fn is_pointer(&self) -> bool;
    /// A wheel step (`wheel`).
    fn wheel(&self) -> Option<Wheel>;
    /// Make this wheel message carry `w` (the sum).
    fn set_wheel(&mut self, w: Wheel);
}

/// The desktop's rule (PRD §6): consecutive pointer moves collapse to the
/// last; consecutive wheel steps of one mode sum. Never across anything
/// else (a button, key or text breaks the run), so order and presses keep.
pub struct DesktopRule;

impl<M: PointerWheel + Send + 'static> Coalesce<M> for DesktopRule {
    fn coalesce(&self, batch: Vec<M>) -> Vec<(usize, M)> {
        let mut out: Vec<(usize, M)> = Vec::with_capacity(batch.len());
        for (i, m) in batch.into_iter().enumerate() {
            if let Some(last) = out.last_mut() {
                if m.is_pointer() && last.1.is_pointer() {
                    *last = (i, m);
                    continue;
                }
                if let (Some(a), Some(b)) = (last.1.wheel(), m.wheel()) {
                    if a.lines == b.lines {
                        last.1.set_wheel(Wheel { dx: a.dx + b.dx, dy: a.dy + b.dy, lines: a.lines });
                        last.0 = i;
                        continue;
                    }
                }
            }
            out.push((i, m));
        }
        out
    }
}

/// Text to push to the viewer at any time (the route's own messages), or
/// a close (a kick, the source switched off).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Push {
    Text(String),
    Close { code: u16, reason: String },
}

/// What the viewer offered (`codecs=`, `win=`, `fps=`, `pw=`, `stats=1`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offer {
    pub codecs: Vec<Codec>,
    pub window: u32,
    pub fps: Option<u32>,
    pub pw: Option<u32>,
    pub stats: bool,
}

impl Offer {
    /// From the query, as the sim reads it: no `codecs` = JPEG, no `win` =
    /// one in flight, an unreadable `fps` = no ask, `pw` 0 = none.
    pub fn from_params(p: &HashMap<String, String>) -> Offer {
        Offer {
            codecs: parse_offer(p.get("codecs").map(String::as_str)),
            window: negotiate_window(p.get("win").map(String::as_str)),
            fps: p.get("fps").and_then(|f| f.parse().ok()),
            pw: p.get("pw").and_then(|v| v.trim().parse::<u32>().ok()).filter(|w| *w > 0),
            stats: p.get("stats").map(String::as_str) == Some("1"),
        }
    }
}

/// What the viewer got (the engine's `hello` fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    pub codec: Codec,
    pub window: u32,
    pub fps: u32,
    pub path: ViewerPath,
}

/// The codec (the first video codec of the offer that `gives` holds, else
/// JPEG), the window and the fps by the rate table.
pub fn negotiate(kind: SourceKind, offer: &Offer, gives: &[Codec], path: ViewerPath) -> Negotiated {
    let codec = negotiate_with(&offer.codecs, gives);
    Negotiated { codec, window: offer.window, fps: rates::clamp_fps(kind, unit_codec(codec), path, offer.fps), path }
}

/// The source a viewer watches.
pub struct Source<E> {
    pub key: SourceKey,
    /// The rate table's row.
    pub kind: SourceKind,
    /// Opens the source if no hub runs for `key` (on a blocking thread).
    pub open: Box<dyn FnOnce() -> Result<Box<dyn EncodedSource>, E> + Send>,
    /// The video codecs it can give: a protocol-4 source's `hello.codecs`;
    /// a pixel source H.264 when the host encodes it, never VP8.
    pub gives: Vec<Codec>,
}

/// The route's side of [`serve`].
pub struct Hooks<M> {
    pub auth: Auth<M>,
    /// Input caps and rate, before the queue (sim: `check_input` +
    /// `InputBudget`; desktop: D1's limits).
    pub limits: InputLimits<M>,
    pub sink: Box<dyn InputSink<M>>,
    pub coalesce: Box<dyn Coalesce<M>>,
    /// The `hello` fields besides the engine's, built once after the
    /// upgrade. The engine's `t`, `codec`, `window`, `fps` and `path` win
    /// a clash.
    pub hello: HelloExtras,
    /// The route's own messages and closes, in order, after `hello`.
    pub push: Option<mpsc::UnboundedReceiver<Push>>,
    /// The answer when the input queue is full (sim: `sim_busy`).
    pub busy: Refusal,
    /// Text to send before the 4410 close when the source ends (sim: a
    /// `state: stopped`), from the source's reason.
    pub ended: EndedText,
    /// The 4410 close's reason (sim: "device stopped").
    pub ended_reason: String,
    /// Log prefix and thread names (`sim` → `[sim]`, `k2-sim-input-<n>`).
    pub tag: &'static str,
}

/// Why [`serve`] didn't upgrade (the route answers over HTTP).
#[derive(Debug)]
pub enum ServeError<E> {
    /// Joining the hub failed (the source couldn't open, or its thread
    /// couldn't start).
    Join(JoinError<E>),
    /// The blocking join task died.
    Task(String),
}

/// The engine's codec for a viewer's.
pub fn unit_codec(c: Codec) -> UnitCodec {
    match c {
        Codec::Jpeg => UnitCodec::Jpeg,
        Codec::H264 => UnitCodec::H264,
        Codec::Vp8 => UnitCodec::Vp8,
    }
}

/// The stream socket before the upgrade: Nagle off, so acks, errors and a
/// frame's tail never wait on a delayed ACK. Loopback and LAN sockets come
/// from the main listener without it; Connect sockets already have it
/// (M-1), so this is a no-op there. `K2_TUNNEL_NODELAY=0` (the existing
/// switch) leaves Nagle on.
pub fn stream_socket_setup(stream: &TcpStream) {
    crate::tunnel_tls_listener::no_nagle(stream);
}

async fn close(write: &mut (impl SinkExt<Message> + Unpin), code: u16, reason: &str) {
    let _ = write
        .send(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: std::borrow::Cow::Owned(reason.to_string()) })))
        .await;
}

fn text(m: &ServerMsg) -> Message {
    Message::Text(serde_json::to_string(m).unwrap_or_default())
}

fn error(r: &Refusal) -> Message {
    text(&ServerMsg::Error { code: r.code.clone(), message: r.message.clone() })
}

fn u16_of(n: u32) -> u16 {
    n.min(u16::MAX as u32) as u16
}

/// A unit as this viewer's K2SF (JPEG) or K2SV (H.264, VP8) message, with
/// its wire `seq`. `ts` counts from `started` (this viewer's stream).
pub fn unit_message(u: &EncodedUnit, seq: u32, started: Instant) -> Vec<u8> {
    let (width_px, height_px, width_pt, height_pt) = (u16_of(u.width), u16_of(u.height), u16_of(u.width_pt), u16_of(u.height_pt));
    let codec = match u.codec {
        UnitCodec::Jpeg => return FrameHeader { seq, width_px, height_px, width_pt, height_pt }.encode(&u.data),
        UnitCodec::H264 => VIDEO_CODEC_H264,
        UnitCodec::Vp8 => VIDEO_CODEC_VP8,
    };
    let age_ms = u.captured_at.elapsed().as_millis().min(u16::MAX as u128) as u16;
    let ts_ms = u.captured_at.saturating_duration_since(started).as_millis().min(u32::MAX as u128) as u32;
    VideoHeader { seq, width_px, height_px, width_pt, height_pt, codec, key: u.key, age_ms, ts_ms }.encode(&u.data)
}

/// One sent unit's stages into the viewer's stats window (M0). The hub
/// gate is the layer's schedule (pulled → its encode began); the queue is
/// encode done → this viewer's write (fan-out, its queue, its window).
fn record_unit(st: &mut StatsWindow, u: &EncodedUnit, send_start: Instant, sent: Instant) {
    let ms = |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f32() * 1000.0;
    if let Some(e) = u.stamps.emitted_at {
        st.stage(Stage::CaptureEmit, ms(u.captured_at, e));
        if let Some(rcv) = u.stamps.received_at {
            st.stage(Stage::EmitReceive, ms(e, rcv));
        }
    }
    st.stage(Stage::ReceiveHub, ms(u.stamps.received_at.unwrap_or(u.captured_at), u.pulled_at));
    match u.encode {
        Some(e) => {
            st.stage(Stage::HubGate, ms(u.pulled_at, e.start));
            st.stage(Stage::Encode, ms(e.start, e.end));
            st.stage(Stage::Queue, ms(e.end, send_start));
        }
        None => st.stage(Stage::Queue, ms(u.pulled_at, send_start)),
    }
    st.stage(Stage::Send, ms(send_start, sent));
    st.stage(Stage::Total, ms(u.captured_at, sent));
    st.frame_sent();
}

/// Send one unit; false when the socket is gone. The caller marks it in
/// flight.
async fn send_unit(write: &mut (impl SinkExt<Message> + Unpin), u: &EncodedUnit, seq: u32, started: Instant, stats: Option<&mut StatsWindow>) -> bool {
    let msg = unit_message(u, seq, started);
    let send_start = Instant::now();
    if write.send(Message::Binary(msg)).await.is_err() {
        return false;
    }
    if let Some(st) = stats {
        record_unit(st, u, send_start, Instant::now());
    }
    true
}

/// CPU ms between two cumulative readings (0 for the first, which has no
/// start).
fn cpu_window_ms(last: &mut u64, now: u64) -> f32 {
    let ms = if *last == 0 { 0.0 } else { now.saturating_sub(*last) as f32 / 1e6 };
    *last = now;
    ms
}

/// May this viewer put one more unit in flight? (The window's delay-based
/// cap: a layer is paced at its source, so a delta that fits the window
/// goes; the round-trip limit would only drop it and cost a keyframe.)
fn room(w: &AckWindow) -> bool {
    (w.in_flight() as u32) < w.cap()
}

/// The `hello` text: the route's fields with the engine's on top.
fn hello_text(mut fields: Map<String, Value>, n: &Negotiated) -> Message {
    fields.insert("t".into(), "hello".into());
    fields.insert("codec".into(), n.codec.as_str().into());
    fields.insert("window".into(), n.window.into());
    fields.insert("fps".into(), n.fps.into());
    fields.insert("path".into(), serde_json::to_value(n.path).unwrap_or(Value::Null));
    Message::Text(Value::Object(fields).to_string())
}

/// Serve one viewer of one source. The route has checked who may connect
/// (before the upgrade) and built its [`Hooks`]. Returns `Err` before the
/// upgrade only (the socket is untouched; the route answers over HTTP);
/// `Ok` once the viewer has come and gone.
///
/// **How the desktop route (D6) wires it**, as a sketch:
///
/// ```ignore
/// let offer = Offer::from_params(&params);
/// let hello = deskd.hello();                     // protocol-4 HelloFields
/// let source = Source {
///     // One capture per desktop (v1): `display` is the one deskd shows now;
///     // a switch (the controller's) moves every viewer of this hub.
///     key: SourceKey::Desktop { desktop: "console".into() },
///     kind: SourceKind::Desktop,
///     open: Box::new(move || {                   // on a blocking thread
///         let sizes: SizesFn = Box::new(move |id| deskd.display_size(id));
///         let send: SendRequest = Box::new(move |r| deskd.send_nr(r));
///         let clock = LinkClock::from_hello(&hello, hello_at);
///         Ok(Box::new(LinkSource::attach(&deskd.feed(), clock, display, sizes, send)) as Box<dyn EncodedSource>)
///     }),
///     gives: hello.codecs.iter().filter_map(|c| Codec::parse(c.as_str())).collect(),
/// };
/// let hooks = Hooks {
///     auth: Auth {
///         every: AUTH_TICK,
///         // Login still valid, role unchanged, grant not expired or revoked.
///         recheck: Box::new(move || match desktop::grant_still_ok(&caller) {
///             Ok(()) => Verdict::Keep,
///             Err(why) => Verdict::Close { code: CLOSE_AUTH_REVOKED, reason: why },
///         }),
///         frame: Box::new(move |f| if incoming.frame_ok(f) { Verdict::Keep } else { access_lost() }),
///         // View-only per message: a Member, `view=1`, a same-machine viewer.
///         may_input: Box::new(move |m| lease.may_drive(&who, m)),
///     },
///     limits: Box::new(move |m, now| desktop::input::check(m).and_then(|_| budget.take(m, now))),
///     sink: Box::new(DeskdSink { deskd }),         // InputSink: deskd.inject(...)
///     coalesce: Box::new(DesktopRule),             // ViewerMsg: PointerWheel
///     hello: Box::new(move |n| desktop_hello_fields(n, &status)), // desktop, display, displays, lane, encoder, mode, …
///     push: Some(cursor_control_and_kick_rx),     // cursor, cursor-pos, control, viewers, Push::Close(4403/4410)
///     busy: Refusal::new("desktop_busy", "Too much input at once; slow down."),
///     ended: Box::new(|why| Some(desktop_state_text("ended", why))),
///     ended_reason: "desktop off".into(),
///     tag: "desktop",
/// };
/// match crate::stream::serve(stream, source, offer, path, hooks).await {
///     Err(ServeError::Join(JoinError::Open(e))) => /* 409 with e */,
///     Err(e) => /* 500 */,
///     Ok(()) => {}
/// }
/// ```
pub async fn serve<M: StreamMsg, E: Send + 'static>(stream: &mut TcpStream, source: Source<E>, offer: Offer, path: ViewerPath, hooks: Hooks<M>) -> Result<(), ServeError<E>> {
    stream_socket_setup(stream);
    let Source { key, kind, open, gives } = source;
    let Hooks { mut auth, mut limits, sink, coalesce, hello, mut push, busy, mut ended, ended_reason, tag } = hooks;
    let neg = negotiate(kind, &offer, &gives, path);
    let (mut codec, mut fps, window_max) = (neg.codec, neg.fps, neg.window);
    let want = Want { codec: unit_codec(codec), fps, path, pw: offer.pw, stats: offer.stats };
    let mut sub = {
        let key = key.clone();
        match tokio::task::spawn_blocking(move || super::join(key, want, open)).await {
            Ok(Ok(sub)) => sub,
            Ok(Err(e)) => return Err(ServeError::Join(e)),
            Err(e) => return Err(ServeError::Task(e.to_string())),
        }
    };
    let ws_conn = match tokio_tungstenite::accept_async(stream).await {
        Ok(w) => w,
        Err(e) => {
            log_debug!("[{tag}] stream handshake failed: {e}");
            return Ok(());
        }
    };
    let (mut write, mut read) = ws_conn.split();
    // M0: stage times once a second, only when asked.
    let mut stats = offer.stats.then(StatsWindow::default);
    let mut hub_cpu_last = 0u64;
    let mut enc_cpu_ns = 0u64;
    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut window = AckWindow::new(window_max);
    let mut congested = false;
    let started = Instant::now();
    let mut gate = ViewerGate::new(sub.layer());
    if write.send(hello_text(hello(&neg), &neg)).await.is_err() {
        return Ok(());
    }
    // Input runs in order on this viewer's own thread (fix 3), so a long
    // gesture never stalls its frames and no move waits for a thread pool;
    // refusals come back on `err_rx`. A probed input (stats) carries its id
    // and read time, and its timing comes back on `timing_rx`.
    let (in_tx, in_rx) = std::sync::mpsc::sync_channel::<Queued<M>>(INPUT_QUEUE);
    let (err_tx, mut err_rx) = mpsc::channel::<Refusal>(16);
    let (timing_tx, mut timing_rx) = mpsc::channel::<InputTiming>(64);
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let me = NEXT.fetch_add(1, Ordering::SeqCst);
    let input = InputThread { sink, coalesce, err_tx, timing_tx, started };
    if let Err(e) = std::thread::Builder::new().name(format!("k2-{tag}-input-{me}")).spawn(move || input.run(in_rx)) {
        log_debug!("[{tag}] input thread for viewer {me}: {e}");
    }
    let mut auth_tick = tokio::time::interval(auth.every);
    auth_tick.tick().await;
    'serve: loop {
        if window.expire(Instant::now(), ACK_EXPIRY) > 0 {
            log_debug!("[{tag}] viewer {me}: acks lost for {ACK_EXPIRY:?}; freeing the window");
        }
        // A keyframe that waited for room goes first.
        if let Some(u) = gate.take_held(room(&window)) {
            let seq = gate.wire_seq(&u);
            if !send_unit(&mut write, &u, seq, started, stats.as_mut()).await {
                break 'serve;
            }
            window.sent(seq, Instant::now());
        }
        tokio::select! {
            _ = auth_tick.tick() => {
                if let Verdict::Close { code, reason } = (auth.recheck)() {
                    log_debug!("[{tag}] closing {key} for viewer {me}: {code} {reason}");
                    close(&mut write, code, &reason).await;
                    break 'serve;
                }
            }
            unit = sub.units.recv() => {
                let Some(u) = unit else {
                    close(&mut write, CLOSE_DEVICE_STOPPED, &ended_reason).await;
                    break 'serve;
                };
                if let Some(e) = &u.encode {
                    enc_cpu_ns += e.cpu_ns;
                }
                match gate.offer(u, room(&window)) {
                    Took::Send(u) => {
                        let seq = gate.wire_seq(&u);
                        if !send_unit(&mut write, &u, seq, started, stats.as_mut()).await {
                            break 'serve;
                        }
                        window.sent(seq, Instant::now());
                    }
                    Took::Dropped => {
                        sub.catch_up();
                    }
                    Took::Held | Took::Stale => {}
                }
            }
            Some(ev) = sub.events.recv() => match ev {
                HubEvent::Moved { layer, fps: f, why, .. } => {
                    log_debug!("[{tag}] {} for viewer {me} on {key} failed ({why}); falling back to JPEG", codec.as_str());
                    gate.moved(layer);
                    codec = Codec::Jpeg;
                    fps = f;
                    let msg = ServerMsg::Codec { codec, message: Some(format!("The video encoder stopped ({why}); this stream is JPEG now.")) };
                    if write.send(text(&msg)).await.is_err() || write.send(text(&ServerMsg::Fps { fps })).await.is_err() {
                        break 'serve;
                    }
                }
                HubEvent::Ended(why) => {
                    if let Some(t) = ended(&why) {
                        let _ = write.send(Message::Text(t)).await;
                    }
                    close(&mut write, CLOSE_DEVICE_STOPPED, &ended_reason).await;
                    break 'serve;
                }
            },
            Some(p) = async { match push.as_mut() { Some(rx) => rx.recv().await, None => std::future::pending().await } } => match p {
                Push::Text(t) => {
                    if write.send(Message::Text(t)).await.is_err() {
                        break 'serve;
                    }
                }
                Push::Close { code, reason } => {
                    close(&mut write, code, &reason).await;
                    break 'serve;
                }
            },
            Some(r) = err_rx.recv() => {
                if write.send(error(&r)).await.is_err() {
                    break 'serve;
                }
            }
            Some(t) = timing_rx.recv() => {
                if let Some(st) = stats.as_mut() {
                    st.input(t);
                }
            }
            _ = stats_tick.tick(), if stats.is_some() => {
                let hub_ms = cpu_window_ms(&mut hub_cpu_last, sub.hub().cpu_ns());
                let enc_ms = std::mem::take(&mut enc_cpu_ns) as f32 / 1e6;
                if let Some(st) = stats.as_mut() {
                    if write.send(text(&ServerMsg::Stats { stats: st.take(hub_ms, enc_ms) })).await.is_err() {
                        break 'serve;
                    }
                }
            }
            msg = read.next() => {
                let acts = matches!(msg, Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))));
                if acts {
                    if let Verdict::Close { code, reason } = (auth.frame)(&msg) {
                        close(&mut write, code, &reason).await;
                        break 'serve;
                    }
                }
                let Some(Ok(msg)) = msg else { break 'serve };
                let Message::Text(t) = msg else {
                    if matches!(msg, Message::Close(_)) { break 'serve; }
                    continue 'serve;
                };
                let Ok(vm) = serde_json::from_str::<M>(&t) else {
                    let _ = write.send(error(&Refusal::new("bad_message", "unknown message"))).await;
                    continue 'serve;
                };
                match vm.engine() {
                    Some(EngineMsg::Ack { seq }) => {
                        window.ack(seq, Instant::now());
                        // The layer's bitrate follows its most congested viewer.
                        if codec.is_video() && window.congested() != congested {
                            congested = !congested;
                            sub.set_congested(congested);
                        }
                    }
                    Some(EngineMsg::Fps { fps: f }) => {
                        fps = rates::clamp_fps(kind, unit_codec(codec), path, Some(f));
                        gate.moved(sub.set_fps(fps));
                        let _ = write.send(text(&ServerMsg::Fps { fps })).await;
                    }
                    Some(EngineMsg::Keyframe) => {
                        if codec.is_video() {
                            sub.ask_keyframe();
                        }
                    }
                    None => {
                        let refused = (auth.may_input)(&vm).and_then(|_| limits(&vm, Instant::now()));
                        if let Err(r) = refused {
                            let _ = write.send(error(&r)).await;
                            continue 'serve;
                        }
                        let probe = stats.as_ref().and(vm.probe()).map(|id| (id, Instant::now()));
                        if in_tx.try_send((vm, probe)).is_err() {
                            let _ = write.send(error(&busy)).await;
                        }
                    }
                }
            }
        }
    }
    drop(in_tx); // the input thread ends after what it is doing
    drop(sub); // leaves the hub: its layer pauses if this was the last viewer
    Ok(())
}

/// One queued input and its stats probe (id, read time).
type Queued<M> = (M, Option<(u32, Instant)>);

/// A viewer's input thread (`k2-<tag>-input-<n>`, fix 3; one per viewer).
/// It takes what is queued, applies the route's [`Coalesce`] rule, and
/// hands each message to the sink in order.
struct InputThread<M> {
    sink: Box<dyn InputSink<M>>,
    coalesce: Box<dyn Coalesce<M>>,
    err_tx: mpsc::Sender<Refusal>,
    timing_tx: mpsc::Sender<InputTiming>,
    started: Instant,
}

impl<M: Send + 'static> InputThread<M> {
    fn run(mut self, rx: std::sync::mpsc::Receiver<Queued<M>>) {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            let (msgs, probes): (Vec<M>, Vec<Option<(u32, Instant)>>) = batch.into_iter().unzip();
            for (i, m) in self.coalesce.coalesce(msgs) {
                let r = self.sink.send(m);
                if let Some(Some((id, recv))) = probes.get(i) {
                    let recv_ms = recv.saturating_duration_since(self.started).as_secs_f32() * 1000.0;
                    let _ = self.timing_tx.try_send(InputTiming { id: *id, recv_ms, lane_ms: recv.elapsed().as_secs_f32() * 1000.0 });
                }
                if let Err(e) = r {
                    if self.err_tx.blocking_send(e).is_err() {
                        return; // the viewer left
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "viewer_tests.rs"]
mod tests;
