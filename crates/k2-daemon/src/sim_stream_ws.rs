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
//! **Engine (S5).** The viewer loop is the stream engine's
//! ([`crate::stream::serve`]); this route gives it the sim's hooks: the
//! token re-check (4401), the login floor per frame, watch-only for Member
//! logins, the keyboard caps and budget, the input sink (`person_input_on`
//! on the viewer's input thread), the sim's coalescing rule
//! ([`SimRule`]), the sim's `hello` fields and its hold changes.
//!
//! **Stats (M0).** `&stats=1` adds a `stats` message once a second with
//! this viewer's per-stage frame times (the encode stage is its layer's
//! encode), its probed inputs and the hub's and its layer's encode CPU
//! time (`k2_core::sim::stream::StatsWindow`). Off unless a viewer asks;
//! the hub samples its CPU time only while one does.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::net::TcpStream;

use k2_core::sim::hold::HoldView;
use k2_core::sim::lane::{Action, FrameOpts, FrameSize, KeyMods, Point, TouchPhase};
use k2_core::sim::manager::{self, Ws};
use k2_core::sim::store::SimRow;
use k2_core::sim::stream::*;
use k2_core::sim::video;
use k2_core::stream::{EncodedSource, SourceKey, SourceKind};

use crate::sidecar_routes::Caller;
use crate::stream::viewer::WsFrame;
use crate::stream::{Auth, Coalesce, EngineMsg, Hooks, InputSink, JoinError, Negotiated, Offer, Push, Refusal, ServeError, Source, StreamMsg, Subscription, Verdict, Want, AUTH_TICK};
#[cfg(test)]
use crate::stream::rates;
#[cfg(test)]
use crate::stream::viewer::{stream_socket_setup, unit_codec};
#[cfg(test)]
use futures_util::{SinkExt, StreamExt};
#[cfg(test)]
use k2_core::stream::{Codec as UnitCodec, EncodedUnit};
#[cfg(test)]
use tokio_tungstenite::tungstenite::Message;

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

/// Opens a running device as the hub's encoded source. S3b: the lane picks
/// (iOS on protocol 4 relays the helper's own units; every other lane goes
/// through `PixelLayers`).
fn open_source(row: SimRow) -> impl FnOnce() -> Result<Box<dyn EncodedSource>, manager::SimError> + Send + 'static {
    move || manager::manager().stream_source(&row, hub_opts(LOCAL_MAX_FPS), hub_frame_size())
}

/// Join (or start) the hub for a running device: subscribed before this
/// returns, so the hub can't end while the socket upgrades.
pub fn join(ws: &Ws, want: Want) -> Result<(i64, Subscription), manager::SimError> {
    let row = manager::manager().running(ws)?;
    let sim_id = row.id;
    match crate::stream::join(SourceKey::Sim(sim_id), want, open_source(row)) {
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

/// The sim row of the rate table (fix 2; the engine clamps with it,
/// `crate::stream::viewer::negotiate`). `video_max` is the local and LAN
/// H.264 cap (`rates::video_max_fps`).
#[cfg(test)]
fn clamp_fps_with(codec: Codec, path: ViewerPath, asked: Option<u32>, video_max: u32) -> u32 {
    rates::clamp_fps_with(SourceKind::Sim, unit_codec(codec), path, asked, video_max)
}

#[cfg(test)]
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

/// The sim's viewer messages on the engine: `ack`, `fps` and `keyframe`
/// are the engine's; a `touch` with an `id` is a stats probe.
impl StreamMsg for ViewerMsg {
    fn engine(&self) -> Option<EngineMsg> {
        match self {
            ViewerMsg::Ack { seq } => Some(EngineMsg::Ack { seq: *seq }),
            ViewerMsg::Fps { fps } => Some(EngineMsg::Fps { fps: *fps }),
            ViewerMsg::Keyframe => Some(EngineMsg::Keyframe),
            _ => None,
        }
    }

    fn probe(&self) -> Option<u32> {
        match self {
            ViewerMsg::Touch { id, .. } => *id,
            _ => None,
        }
    }
}

/// The sim's coalescing rule (fix 3): backed-up moves collapse to the
/// newest ([`coalesce_keep`]); nothing else merges.
pub struct SimRule;

impl Coalesce<ViewerMsg> for SimRule {
    fn coalesce(&self, batch: Vec<ViewerMsg>) -> Vec<(usize, ViewerMsg)> {
        let keep = coalesce_keep(&batch);
        batch.into_iter().zip(keep).enumerate().filter_map(|(i, (m, k))| k.then_some((i, m))).collect()
    }
}

/// A viewer's input into the device, as `k2 sim` would send it.
struct SimSink {
    sim_id: i64,
    ws: Ws,
    who: String,
    caller: Caller,
}

impl InputSink<ViewerMsg> for SimSink {
    fn send(&mut self, vm: ViewerMsg) -> Result<(), Refusal> {
        person_input_on(Some(self.sim_id), &self.ws, &self.who, &self.caller, vm).map_err(|e| Refusal::new(e.code, e.message))
    }
}

/// Hold changes (a tap from another viewer, take-over expiry), polled as
/// before (every 250 ms) and pushed to the viewer while it stays.
fn hold_changes(sim_id: i64, mut last: HoldView) -> tokio::sync::mpsc::UnboundedReceiver<Push> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        while !tx.is_closed() {
            tick.tick().await;
            let h = manager::manager().hold_view(sim_id);
            let changed_kind = std::mem::discriminant(&h) != std::mem::discriminant(&last)
                || matches!((&h, &last), (HoldView::Takeover { by: a, .. }, HoldView::Takeover { by: b, .. }) if a != b);
            if changed_kind {
                last = h.clone();
                if tx.send(Push::Text(serde_json::to_string(&ServerMsg::Hold { hold: h }).unwrap_or_default())).is_err() {
                    return;
                }
            }
        }
    });
    rx
}

/// A server message's fields (for the engine's `hello`).
fn fields(m: &ServerMsg) -> Map<String, Value> {
    match serde_json::to_value(m) {
        Ok(Value::Object(o)) => o,
        _ => Map::new(),
    }
}

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
    // Codec + window (0.46.7). A viewer that offers nothing gets JPEG with
    // one frame in flight: the wire as it was. A sim gives H.264 when this
    // host encodes it, never VP8.
    let offer = Offer::from_params(&params);
    let host_h264 = offer.codecs.contains(&Codec::H264) && tokio::task::spawn_blocking(video::h264_available).await.unwrap_or(false);
    let gives = if host_h264 { vec![Codec::H264] } else { Vec::new() };
    let running = {
        let ws2 = ws.clone();
        tokio::task::spawn_blocking(move || {
            let m = manager::manager();
            let row = m.running(&ws2)?;
            Ok::<_, manager::SimError>((row, m.status(&ws2, false).ok().and_then(|s| s.sim)))
        })
        .await
    };
    let (row, view) = match running {
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
    let sim_id = row.id;
    let m = manager::manager();
    let hold = m.hold_view(sim_id);
    let input_caps = view.as_ref().map(|v| m.input_caps(v.lane));
    let can_control = crate::sim_routes::can_control(&viewer.caller);
    let mut incoming = crate::socket_access::IncomingCheck::new(&viewer.token, &owner_token);
    let mut budget = InputBudget::new(Instant::now());
    let (tok, owner) = (viewer.token.clone(), owner_token.clone());
    let hello_hold = hold.clone();
    let hooks = Hooks {
        auth: Auth {
            every: AUTH_TICK,
            recheck: Box::new(move || {
                if crate::routes::http::token_still_valid(&tok, &owner) {
                    Verdict::Keep
                } else {
                    Verdict::Close { code: CLOSE_AUTH_REVOKED, reason: "token revoked".into() }
                }
            }),
            // Security review L7: a client frame from a login is acted on
            // only while the login still clears this socket's floor (a
            // downgrade to Voice & Chat only closes it here, not on the tick).
            frame: Box::new(move |f: &WsFrame| {
                if incoming.frame_ok(f) {
                    Verdict::Keep
                } else {
                    Verdict::Close { code: crate::presence::WS_CLOSE_ACCESS_LOST, reason: "access_lost".into() }
                }
            }),
            may_input: Box::new(move |_: &ViewerMsg| if can_control { Ok(()) } else { Err(Refusal::new("sim_watch_only", "You can watch this device but not use it.")) }),
        },
        limits: Box::new(move |m: &ViewerMsg, now: Instant| check_input(m).and_then(|_| budget.take(m, now)).map_err(|r| Refusal::new(r.code, r.message))),
        sink: Box::new(SimSink { sim_id, ws: ws.clone(), who: who_name(&viewer.caller), caller: viewer.caller.clone() }),
        coalesce: Box::new(SimRule),
        hello: Box::new(move |n: &Negotiated| {
            fields(&ServerMsg::Hello {
                sim: format!("s{sim_id}"),
                lane: view.as_ref().map(|v| v.lane).unwrap_or(k2_core::sim::LaneKind::Android),
                state: view.as_ref().map(|v| v.state).unwrap_or(SimState::Running),
                profile: view.as_ref().and_then(|v| v.profile.clone()),
                fps: n.fps,
                remote: n.path.remote(),
                path: Some(n.path),
                hold: hello_hold,
                isolation: k2_core::sim::ISOLATION_NOTE.to_string(),
                input: input_caps,
                codec: n.codec,
                window: n.window,
            })
        }),
        push: Some(hold_changes(sim_id, hold)),
        busy: Refusal::new("sim_busy", "Too much input at once; slow down."),
        ended: Box::new(|why: &str| serde_json::to_string(&ServerMsg::State { state: SimState::Stopped, message: Some(why.to_string()) }).ok()),
        ended_reason: "device stopped".into(),
        tag: "sim",
    };
    let source = Source { key: SourceKey::Sim(sim_id), kind: SourceKind::Sim, open: Box::new(open_source(row)), gives };
    m.viewer_joined(sim_id);
    let served = crate::stream::serve(stream, source, offer, viewer.path, hooks).await;
    m.viewer_left(sim_id);
    let refused = match served {
        Ok(()) => return,
        Err(ServeError::Join(JoinError::Open(e))) => e,
        Err(ServeError::Join(JoinError::Thread(e))) => manager::SimError::new(500, "sim_io", e),
        Err(ServeError::Task(e)) => {
            crate::routes::http::send_response(&mut *stream, "500 Internal Server Error", "application/json", &format!("{{\"error\":\"{e}\"}}")).await;
            return;
        }
    };
    crate::routes::http::send_response(&mut *stream, "409 Conflict", "application/json", &refused.body().to_string()).await;
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
