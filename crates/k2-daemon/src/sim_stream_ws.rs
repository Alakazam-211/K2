//! `GET /cli/sim/stream` (prd-mobile-dev-v1 §7, S3; vs-live M-A4, M-A19):
//! the live device over a WebSocket, plus people's input. Wire format:
//! `k2_core::sim::stream`.
//!
//! **One capture per device, one encode per layer.** The device's frames go
//! through the stream engine (`crate::stream`, fix 10): one hub per sim
//! session (`SourceKey::Sim`), whose capture thread pulls the lane's
//! `FrameSource` through a `PixelLayers` adapter and encodes each frame
//! once per layer (codec, fps tier, local or remote), never per viewer:
//! H.264 through VideoToolbox on a Mac, JPEG elsewhere (a lane JPEG that
//! fits streams as it is). Each layer keeps its own fixed schedule; the
//! lane is retuned to the fastest layer. It asks the lane for points-sized
//! frames (fix 1a) until a viewer sends its pane width (`&pw=<device px>`),
//! then for native pixels capped at the widest pane on the layer.
//! `K2_SIM_FRAME_SIZE=native` (read once) asks for native frames, as before.
//!
//! **Flow.** Each viewer task sends its layer's units as they come while
//! its ack window has room (`k2_simvideo::flow::AckWindow`: up to the
//! negotiated `win`). A viewer that offers nothing gets one frame in
//! flight, as before: a JPEG unit is always a keyframe, so the newest is
//! held and goes on the ack. **Never a delta after a gap:** an H.264 delta
//! a viewer can't take (no room, or its queue lost units) is dropped and
//! the viewer waits for the next keyframe, asking the hub for one (once a
//! second per layer). Keyframes come on a layer's start, on a size change
//! (rotation, a wider pane), for a viewer joining a running layer (never
//! throttled) and when a viewer asks (`{"t":"keyframe"}`, 250 ms apart per
//! layer). A layer's bitrate follows its size and fps and steps down while
//! its most congested viewer's acks show a queue building.
//!
//! **Auth.** People only: the owner token or a Connect login (re-checked
//! every 5 s, like `/cli/sessions/bytes`; a revoked login is closed with
//! 4401). App passes, API keys and agent passports never upgrade (agents
//! read the tree and take screenshots instead). Input needs control: the
//! owner and Owner/Admin logins; Member logins watch only.
//!
//! **Rates** ([`clamp_fps`], `crate::stream::rates`, by codec and
//! [`ViewerPath`]): JPEG up to 30 fps on this machine, 2–15 on the LAN or
//! over Connect (default 8); H.264 up to 60 on this machine, 2–60 on the
//! LAN (default 60), 2–30 over Connect (default 24).
//! `K2_SIM_VIDEO_MAX_FPS=30` (read once) puts the local and LAN H.264 caps
//! back to 30. A layer runs at the viewer's rate rounded up to a tier
//! (60, 30, 15, 8).
//!
//! **Stats (M0).** `&stats=1` adds a `stats` message once a second with
//! this viewer's per-stage frame times (the encode stage is its layer's
//! encode), its probed inputs and the hub's and its layer's encode CPU
//! time (`k2_core::sim::stream::StatsWindow`). Off unless a viewer asks;
//! the hub samples its CPU time only while one does.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

use k2_core::log_debug;
use k2_core::sim::hold::HoldView;
use k2_core::sim::lane::{Action, FrameOpts, FrameSize, KeyMods, Point, TouchPhase};
use k2_core::sim::manager::{self, Ws};
use k2_core::sim::stream::*;
use k2_core::sim::video::flow::{AckWindow, ACK_EXPIRY};
use k2_core::sim::video;
use k2_core::stream::{Codec as UnitCodec, EncodedSource, EncodedUnit, SourceKey, SourceKind};

use crate::sidecar_routes::Caller;
use crate::stream::{rates, HubEvent, JoinError, Subscription, Took, ViewerGate, Want};

/// The size the hub asks lanes for: points, unless `K2_SIM_FRAME_SIZE=native`
/// (read once at the first stream; the fix 1a rollback).
fn hub_frame_size() -> FrameSize {
    static SIZE: OnceLock<FrameSize> = OnceLock::new();
    *SIZE.get_or_init(|| match std::env::var("K2_SIM_FRAME_SIZE") {
        Ok(v) if v.trim().eq_ignore_ascii_case("native") => FrameSize::Native,
        _ => FrameSize::Points,
    })
}

/// What the hub opens its source with at `fps`.
fn hub_opts(fps: u32) -> FrameOpts {
    FrameOpts { max_fps: fps, size: hub_frame_size(), max_width_px: None }
}

/// The stream engine's codec for a viewer's.
fn unit_codec(c: Codec) -> UnitCodec {
    match c {
        Codec::Jpeg => UnitCodec::Jpeg,
        Codec::H264 => UnitCodec::H264,
    }
}

/// Join (or start) the hub for a running device: subscribed before this
/// returns, so the hub can't end while the socket upgrades.
pub fn join(ws: &Ws, want: Want) -> Result<(i64, Subscription), manager::SimError> {
    let m = manager::manager();
    let row = m.running(ws)?;
    let sim_id = row.id;
    // S3b: the lane picks (iOS on protocol 4 relays the helper's own units;
    // every other lane goes through `PixelLayers`).
    let open = || -> Result<Box<dyn EncodedSource>, manager::SimError> { m.stream_source(&row, hub_opts(LOCAL_MAX_FPS), hub_frame_size()) };
    match crate::stream::join(SourceKey::Sim(sim_id), want, open) {
        Ok(sub) => Ok((sim_id, sub)),
        Err(JoinError::Open(e)) => Err(e),
        Err(JoinError::Thread(e)) => Err(manager::SimError::new(500, "sim_io", e)),
    }
}

/// Who is watching (decided by the dispatcher from the credential), and
/// how they reach us (from the ingress).
#[derive(Debug, Clone)]
pub struct Viewer {
    pub caller: Caller,
    pub token: String,
    pub path: ViewerPath,
}

/// The sim row of the rate table (fix 2). `video_max` is the local and LAN
/// H.264 cap (`rates::video_max_fps`).
fn clamp_fps_with(codec: Codec, path: ViewerPath, asked: Option<u32>, video_max: u32) -> u32 {
    rates::clamp_fps_with(SourceKind::Sim, unit_codec(codec), path, asked, video_max)
}

fn clamp_fps(codec: Codec, path: ViewerPath, asked: Option<u32>) -> u32 {
    clamp_fps_with(codec, path, asked, rates::video_max_fps())
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

fn u16_of(n: u32) -> u16 {
    n.min(u16::MAX as u32) as u16
}

/// A unit as this viewer's K2SF (JPEG) or K2SV (H.264) message, with its
/// wire `seq`. `ts` counts from `started` (this viewer's stream). `None`
/// for a codec the sim wire doesn't carry.
fn unit_message(u: &EncodedUnit, seq: u32, started: Instant) -> Option<Vec<u8>> {
    let (width_px, height_px, width_pt, height_pt) = (u16_of(u.width), u16_of(u.height), u16_of(u.width_pt), u16_of(u.height_pt));
    match u.codec {
        UnitCodec::Jpeg => Some(FrameHeader { seq, width_px, height_px, width_pt, height_pt }.encode(&u.data)),
        UnitCodec::H264 => {
            let age_ms = u.captured_at.elapsed().as_millis().min(u16::MAX as u128) as u16;
            let ts_ms = u.captured_at.saturating_duration_since(started).as_millis().min(u32::MAX as u128) as u32;
            Some(VideoHeader { seq, width_px, height_px, width_pt, height_pt, codec: VIDEO_CODEC_H264, key: u.key, age_ms, ts_ms }.encode(&u.data))
        }
        UnitCodec::Vp8 => None,
    }
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
    let Some(msg) = unit_message(u, seq, started) else { return true };
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

/// The stream socket before the upgrade: Nagle off, so acks, hold and
/// error messages and a frame's tail never wait on a delayed ACK. Loopback
/// and LAN sockets come from the main listener without it; Connect sockets
/// already have it (M-1), so this is a no-op there. `K2_TUNNEL_NODELAY=0`
/// (the existing switch) leaves Nagle on.
fn stream_socket_setup(stream: &TcpStream) {
    crate::tunnel_tls_listener::no_nagle(stream);
}

/// May this viewer put one more unit in flight? (The window's delay-based
/// cap: a layer is paced at its source, so a delta that fits the window
/// goes; the round-trip limit would only drop it and cost a keyframe.)
fn room(w: &AckWindow) -> bool {
    (w.in_flight() as u32) < w.cap()
}

/// Serve one viewer. The dispatcher has refused app passes, agents and bad
/// tokens before the upgrade.
pub async fn serve(stream: &mut TcpStream, params: HashMap<String, String>, viewer: Viewer, owner_token: String) {
    stream_socket_setup(stream);
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
    // Codec + window (0.46.7). A viewer that offers nothing gets JPEG with
    // one frame in flight: the wire as it was.
    let offer = parse_offer(params.get("codecs").map(String::as_str));
    let window_max = negotiate_window(params.get("win").map(String::as_str));
    let host_h264 = offer.contains(&Codec::H264) && tokio::task::spawn_blocking(video::h264_available).await.unwrap_or(false);
    let mut codec = negotiate(&offer, host_h264);
    let remote = viewer.path.remote();
    let mut fps = clamp_fps(codec, viewer.path, params.get("fps").and_then(|f| f.parse().ok()));
    // The pane width in device pixels: the layer goes up to it (clamped to
    // the device's own width). Absent: points, as before.
    let pw = params.get("pw").and_then(|v| v.trim().parse::<u32>().ok()).filter(|w| *w > 0);
    let stats_on = params.get("stats").map(String::as_str) == Some("1");
    let want = Want { codec: unit_codec(codec), fps, path: viewer.path, pw, stats: stats_on };
    let joined = {
        let ws2 = ws.clone();
        tokio::task::spawn_blocking(move || join(&ws2, want)).await
    };
    let (sim_id, mut sub) = match joined {
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
    // M0: stage times once a second, only when asked.
    let mut stats = stats_on.then(StatsWindow::default);
    let mut hub_cpu_last = 0u64;
    let mut enc_cpu_ns = 0u64;
    let mut stats_tick = tokio::time::interval(Duration::from_secs(1));
    stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
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
        remote,
        path: Some(viewer.path),
        hold: m.hold_view(sim_id),
        isolation: k2_core::sim::ISOLATION_NOTE.to_string(),
        input: view.as_ref().map(|v| m.input_caps(v.lane)),
        codec,
        window: window_max,
    };
    let mut window = AckWindow::new(window_max);
    let mut congested = false;
    let started = Instant::now();
    let mut gate = ViewerGate::new(sub.layer());
    let mut last_hold = m.hold_view(sim_id);
    if write.send(text(&hello)).await.is_err() {
        drop(sub);
        m.viewer_left(sim_id);
        return;
    }
    // People's input runs in order on this viewer's own thread (fix 3), so
    // a long swipe never stalls its frames and no move waits for a thread
    // pool; refusals come back on `err_rx`. A probed touch (stats) carries
    // its id and read time, and its timing comes back on `timing_rx`.
    let (in_tx, in_rx) = std::sync::mpsc::sync_channel::<QueuedInput>(INPUT_QUEUE);
    let (err_tx, mut err_rx) = tokio::sync::mpsc::channel::<(String, String)>(16);
    let (timing_tx, mut timing_rx) = tokio::sync::mpsc::channel::<InputTiming>(64);
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let me = NEXT.fetch_add(1, Ordering::SeqCst);
    let input = InputThread { sim_id, ws: ws.clone(), who: who.clone(), caller: viewer.caller.clone(), err_tx, timing_tx, started };
    if let Err(e) = std::thread::Builder::new().name(format!("k2-sim-input-{me}")).spawn(move || input.run(in_rx)) {
        log_debug!("[sim] input thread for viewer {me}: {e}");
    }
    let mut budget = InputBudget::new(Instant::now());
    let mut auth = tokio::time::interval(Duration::from_secs(5));
    auth.tick().await;
    // Hold changes (a tap from another viewer, take-over expiry).
    let mut hold_tick = tokio::time::interval(Duration::from_millis(250));
    hold_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut incoming = crate::socket_access::IncomingCheck::new(&viewer.token, &owner_token);
    'serve: loop {
        if window.expire(Instant::now(), ACK_EXPIRY) > 0 {
            log_debug!("[sim] viewer {me}: acks lost for {ACK_EXPIRY:?}; freeing the window");
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
            _ = auth.tick() => {
                if !crate::routes::http::token_still_valid(&viewer.token, &owner_token) {
                    log_debug!("[sim] stream token revoked; closing s{sim_id}");
                    close(&mut write, CLOSE_AUTH_REVOKED, "token revoked").await;
                    break 'serve;
                }
            }
            unit = sub.units.recv() => {
                let Some(u) = unit else {
                    close(&mut write, CLOSE_DEVICE_STOPPED, "device stopped").await;
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
                    log_debug!("[sim] H.264 for viewer {me} on s{sim_id} failed ({why}); falling back to JPEG");
                    gate.moved(layer);
                    codec = Codec::Jpeg;
                    fps = f;
                    let msg = ServerMsg::Codec { codec, message: Some(format!("The video encoder stopped ({why}); this stream is JPEG now.")) };
                    if write.send(text(&msg)).await.is_err() || write.send(text(&ServerMsg::Fps { fps })).await.is_err() {
                        break 'serve;
                    }
                }
                HubEvent::Ended(why) => {
                    let _ = write.send(text(&ServerMsg::State { state: SimState::Stopped, message: Some(why) })).await;
                    close(&mut write, CLOSE_DEVICE_STOPPED, "device stopped").await;
                    break 'serve;
                }
            },
            _ = hold_tick.tick() => {
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
                // Security review L7: a client frame from a login is acted on
                // only while the login still clears this socket's floor (a
                // downgrade to Voice & Chat only closes it here, not on the tick).
                if !incoming.frame_ok(&msg) {
                    let _ = write.send(crate::presence::access_lost_close_message()).await;
                    break 'serve;
                }
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
                        window.ack(seq, Instant::now());
                        // The layer's bitrate follows its most congested viewer.
                        if codec == Codec::H264 && window.congested() != congested {
                            congested = !congested;
                            sub.set_congested(congested);
                        }
                    }
                    ViewerMsg::Fps { fps: f } => {
                        fps = clamp_fps(codec, viewer.path, Some(f));
                        gate.moved(sub.set_fps(fps));
                        let _ = write.send(text(&ServerMsg::Fps { fps })).await;
                    }
                    ViewerMsg::Keyframe => {
                        if codec == Codec::H264 {
                            sub.ask_keyframe();
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
                        let probe = match (&other, stats.is_some()) {
                            (ViewerMsg::Touch { id: Some(id), .. }, true) => Some((*id, Instant::now())),
                            _ => None,
                        };
                        if in_tx.try_send((other, probe)).is_err() {
                            let _ = write.send(text(&ServerMsg::Error { code: "sim_busy".into(), message: "Too much input at once; slow down.".into() })).await;
                        }
                    }
                }
            }
        }
    }
    drop(in_tx); // the input thread ends after what it is doing
    drop(sub); // leaves the hub: its layer pauses if this was the last viewer
    m.viewer_left(sim_id);
}

/// Inputs a viewer may have queued; one more is refused with `sim_busy`.
const INPUT_QUEUE: usize = 64;

/// One queued input and its stats probe (id, read time).
type QueuedInput = (ViewerMsg, Option<(u32, Instant)>);

/// A viewer's input thread (`k2-sim-input-<n>`, fix 3; K2 Q3: one per
/// viewer). It takes what is queued, collapses backed-up moves to the
/// newest (`coalesce_keep`), and sends each in order. Per viewer, not per
/// device, so today's ordering and lifetime rules hold; a per-device input
/// thread belongs to the shared engine.
struct InputThread {
    sim_id: i64,
    ws: Ws,
    who: String,
    caller: Caller,
    err_tx: tokio::sync::mpsc::Sender<(String, String)>,
    timing_tx: tokio::sync::mpsc::Sender<InputTiming>,
    started: Instant,
}

impl InputThread {
    fn run(self, rx: std::sync::mpsc::Receiver<QueuedInput>) {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            batch.extend(rx.try_iter());
            let keep = coalesce_keep(&batch.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>());
            for ((vm, probe), _) in batch.into_iter().zip(keep).filter(|(_, k)| *k) {
                let r = person_input_on(Some(self.sim_id), &self.ws, &self.who, &self.caller, vm);
                if let Some((id, recv)) = probe {
                    let recv_ms = recv.saturating_duration_since(self.started).as_secs_f32() * 1000.0;
                    let _ = self.timing_tx.try_send(InputTiming { id, recv_ms, lane_ms: recv.elapsed().as_secs_f32() * 1000.0 });
                }
                if let Err(e) = r {
                    if self.err_tx.blocking_send((e.code.to_string(), e.message)).is_err() {
                        return; // the viewer left
                    }
                }
            }
        }
    }
}

/// One person input from the stream (blocking), the way `k2 sim` would
/// send it: every input reads the row (tests).
#[cfg(test)]
fn person_input(ws: &Ws, who: &str, caller: &Caller, vm: ViewerMsg) -> Result<(), manager::SimError> {
    person_input_on(None, ws, who, caller, vm)
}

/// One person input; with the stream's device id, a gesture's moves take
/// the manager's in-memory fast path (fix 3).
fn person_input_on(sim_id: Option<i64>, ws: &Ws, who: &str, caller: &Caller, vm: ViewerMsg) -> Result<(), manager::SimError> {
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
        ViewerMsg::Touch { phase: TouchPhase::Up, x, y, x2: None, y2: None, vx: Some(vx), vy: Some(vy), .. } => {
            m.person_fling(ws, who, Point { x, y }, (vx, vy))
        }
        o => match (to_action(&o), sim_id) {
            (Some(a), Some(id)) => m.person_action_sim(id, ws, who, &a),
            (Some(a), None) => m.person_action(ws, who, &a),
            (None, _) => Err(manager::SimError::new(400, "sim_invalid", "bad input")),
        },
    }
}

#[cfg(test)]
#[path = "sim_stream_ws_tests.rs"]
mod tests;
