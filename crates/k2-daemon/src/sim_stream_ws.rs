//! `GET /cli/sim/stream` (prd-mobile-dev-v1 §7, S3; vs-live M-A4, M-A19):
//! the live device as JPEG frames over a WebSocket, plus people's input.
//! Wire format: `k2_core::sim::stream`.
//!
//! **One capture per device, many viewers.** A [`Hub`] per sim session
//! runs one encoder thread: it pulls the lane's [`FrameSource`], encodes a
//! points-size JPEG and publishes it into a `watch` channel, which keeps
//! only the newest frame. Each viewer task sends the newest frame when its
//! last one is acked (one in flight) and its fps cap allows, so a slow
//! viewer skips frames and never holds the others back. The hub stops when
//! its last viewer leaves.
//!
//! **Auth.** People only: the owner token or a Connect login (re-checked
//! every 5 s, like `/cli/sessions/bytes`; a revoked login is closed with
//! 4401). App passes, API keys and agent passports never upgrade (agents
//! read the tree and take screenshots instead). Input needs control: the
//! owner and Owner/Admin logins; Member logins watch only.
//!
//! **Rates.** Loopback viewers up to 30 fps (default 30); remote viewers
//! (LAN / Connect) 8 fps by default, 2–15 on request.

use std::collections::HashMap;
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
use k2_core::sim::lane::{Action, FrameOpts, KeyMods, Point, TouchPhase};
use k2_core::sim::manager::{self, Ws};
use k2_core::sim::stream::*;

use crate::sidecar_routes::Caller;

/// One encoded frame, shared by every viewer.
#[derive(Debug)]
pub struct Encoded {
    pub seq: u32,
    /// Header + JPEG, ready to send.
    pub message: Vec<u8>,
}

/// What the encoder publishes.
#[derive(Debug, Clone)]
pub enum Pub {
    Starting,
    Frame(Arc<Encoded>),
    /// The device stopped or the capture failed for good.
    Ended(String),
}

/// One device's capture.
pub struct Hub {
    gen: u64,
    rx: watch::Receiver<Pub>,
    /// Highest fps any viewer asked for (the encoder's pace).
    want_fps: Arc<Mutex<HashMap<u64, u32>>>,
    encodes: Arc<std::sync::atomic::AtomicU64>,
}

fn hubs() -> &'static Mutex<HashMap<i64, Arc<Hub>>> {
    static H: OnceLock<Mutex<HashMap<i64, Arc<Hub>>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many JPEG encodes a device's hub has done (tests: one encode per
/// frame however many viewers).
pub fn encodes(sim_id: i64) -> Option<u64> {
    hubs().lock().ok()?.get(&sim_id).map(|h| h.encodes.load(std::sync::atomic::Ordering::SeqCst))
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
    let encodes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let gen = GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let hub = Arc::new(Hub { gen, rx, want_fps: want_fps.clone(), encodes: encodes.clone() });
    map.insert(row.id, hub.clone());
    let sim_id = row.id;
    std::thread::Builder::new()
        .name(format!("k2-sim-encode-{sim_id}"))
        .spawn(move || encoder(sim_id, gen, source, tx, want_fps, encodes))
        .map_err(|e| manager::SimError::new(500, "sim_io", format!("encoder thread: {e}")))?;
    Ok((sim_id, hub))
}

fn encoder(
    sim_id: i64,
    gen: u64,
    mut source: Box<dyn k2_core::sim::lane::FrameSource>,
    tx: watch::Sender<Pub>,
    want_fps: Arc<Mutex<HashMap<u64, u32>>>,
    encodes: Arc<std::sync::atomic::AtomicU64>,
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
                let (jpeg, w, h) = match encode::frame_jpeg(&f, encode::JPEG_QUALITY) {
                    Ok(x) => x,
                    Err(e) => {
                        log_debug!("[sim] encode failed on s{sim_id}: {e}");
                        continue;
                    }
                };
                encodes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                seq = seq.wrapping_add(1);
                // Points: the frame is points-sized (scale 1) unless the lane
                // says otherwise.
                let (wpt, hpt) = if (f.scale - 1.0).abs() < 1e-3 {
                    (w, h)
                } else {
                    let p = encode::points_size(&f);
                    (p.0, p.1)
                };
                let header = FrameHeader { seq, width_px: w as u16, height_px: h as u16, width_pt: wpt as u16, height_pt: hpt as u16 };
                let _ = tx.send(Pub::Frame(Arc::new(Encoded { seq, message: header.encode(&jpeg) })));
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

/// Who is watching (decided by the dispatcher from the credential).
#[derive(Debug, Clone)]
pub struct Viewer {
    pub caller: Caller,
    pub token: String,
    pub remote: bool,
}

fn clamp_fps(remote: bool, asked: Option<u32>) -> u32 {
    if remote {
        asked.unwrap_or(REMOTE_DEFAULT_FPS).clamp(REMOTE_MIN_FPS, REMOTE_MAX_FPS)
    } else {
        asked.unwrap_or(LOCAL_MAX_FPS).clamp(1, LOCAL_MAX_FPS)
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
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let me = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let mut fps = clamp_fps(viewer.remote, params.get("fps").and_then(|f| f.parse().ok()));
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
    };
    let mut rx = hub.rx.clone();
    let mut in_flight = false;
    let mut last_sent = Instant::now() - Duration::from_secs(1);
    let mut last_hold = m.hold_view(sim_id);
    if write.send(text(&hello)).await.is_err() {
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
    loop {
        // Send the newest frame when allowed (one in flight, fps cap).
        if pending && !in_flight && last_sent.elapsed() >= Duration::from_millis(1000 / fps as u64) {
            let p = rx.borrow_and_update().clone();
            match p {
                Pub::Frame(f) => {
                    if write.send(Message::Binary(f.message.clone())).await.is_err() {
                        break;
                    }
                    in_flight = true;
                    last_sent = Instant::now();
                    pending = false;
                }
                Pub::Ended(why) => {
                    let _ = write.send(text(&ServerMsg::State { state: SimState::Stopped, message: Some(why) })).await;
                    close(&mut write, CLOSE_DEVICE_STOPPED, "device stopped").await;
                    break;
                }
                Pub::Starting => pending = false,
            }
        }
        tokio::select! {
            _ = auth.tick() => {
                if !crate::routes::http::token_still_valid(&viewer.token, &owner_token) {
                    log_debug!("[sim] stream token revoked; closing s{sim_id}");
                    close(&mut write, CLOSE_AUTH_REVOKED, "token revoked").await;
                    break;
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    close(&mut write, CLOSE_DEVICE_STOPPED, "device stopped").await;
                    break;
                }
                pending = true;
            }
            _ = tokio::time::sleep(if pending && !in_flight {
                Duration::from_millis(1000 / fps as u64).saturating_sub(last_sent.elapsed()).max(Duration::from_millis(2))
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
                        break;
                    }
                }
            }
            Some((code, message)) = err_rx.recv() => {
                if write.send(text(&ServerMsg::Error { code, message })).await.is_err() {
                    break;
                }
            }
            msg = read.next() => {
                let Some(Ok(msg)) = msg else { break };
                let Message::Text(t) = msg else {
                    if matches!(msg, Message::Close(_)) { break; }
                    continue;
                };
                let Ok(vm) = serde_json::from_str::<ViewerMsg>(&t) else {
                    let _ = write.send(text(&ServerMsg::Error { code: "bad_message".into(), message: "unknown message".into() })).await;
                    continue;
                };
                match vm {
                    ViewerMsg::Ack { seq } => {
                        let _ = seq;
                        in_flight = false;
                    }
                    ViewerMsg::Fps { fps: f } => {
                        fps = clamp_fps(viewer.remote, Some(f));
                        if let Ok(mut w) = hub.want_fps.lock() { w.insert(me, fps); }
                        let _ = write.send(text(&ServerMsg::Fps { fps })).await;
                    }
                    other => {
                        if !can_control {
                            let _ = write.send(text(&ServerMsg::Error { code: "sim_watch_only".into(), message: "You can watch this device but not use it.".into() })).await;
                            continue;
                        }
                        if let Err(r) = check_input(&other).and_then(|_| budget.take(&other, Instant::now())) {
                            let _ = write.send(text(&ServerMsg::Error { code: r.code.into(), message: r.message })).await;
                            continue;
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
    if let Ok(mut w) = hub.want_fps.lock() {
        w.remove(&me);
    }
    drop(rx);
    m.viewer_left(sim_id);
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
mod tests {
    use super::*;

    #[test]
    fn fps_caps_follow_local_and_remote() {
        assert_eq!(clamp_fps(false, None), 30);
        assert_eq!(clamp_fps(false, Some(60)), 30);
        assert_eq!(clamp_fps(true, None), 8);
        assert_eq!(clamp_fps(true, Some(1)), 2);
        assert_eq!(clamp_fps(true, Some(30)), 15);
    }

    /// Two viewers share one hub and one encode per frame; a viewer that
    /// never reads holds nobody back (the channel keeps only the newest).
    #[test]
    fn one_encoder_serves_every_viewer() {
        let _serial = crate::sim_routes::sim_test_lock();
        let tmp = std::env::temp_dir().join(format!("k2-sim-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("ws")).expect("ws");
        k2_core::sim::set_base_for_tests(tmp.join("home"));
        k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
        let fake = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Android));
        k2_core::sim::register_lane(fake);
        let m = manager::manager();
        m.refresh_probes();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'stream', ?2)", rusqlite::params![id, tmp.join("ws").display().to_string()])
                .expect("project");
        }
        let ws = Ws { id: id.clone(), handle: "stream".into(), path: tmp.join("ws") };
        m.boot(&ws, &manager::Actor::Person { name: "owner".into() }, &k2_core::sim::api::BootReq { no_tab: true, ..Default::default() }).expect("boot");
        let (sim_a, hub_a) = join(&ws).expect("join a");
        let (sim_b, hub_b) = join(&ws).expect("join b");
        assert_eq!(sim_a, sim_b);
        assert!(Arc::ptr_eq(&hub_a, &hub_b), "one hub per device");
        hub_a.want_fps.lock().expect("fps").insert(1, 30);
        let mut fast = hub_a.rx.clone();
        let _slow = hub_b.rx.clone(); // never reads
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("rt");
        let mut got = Vec::new();
        rt.block_on(async {
            while got.len() < 5 {
                tokio::time::timeout(Duration::from_secs(5), fast.changed()).await.expect("a frame within 5 s").expect("hub alive");
                if let Pub::Frame(f) = &*fast.borrow_and_update() {
                    got.push(f.clone());
                }
            }
        });
        let seqs: Vec<u32> = got.iter().map(|f| f.seq).collect();
        assert!(seqs.windows(2).all(|w| w[1] > w[0]), "{seqs:?}");
        let (h, jpeg) = FrameHeader::decode(&got[0].message).expect("header");
        assert_eq!((h.width_px, h.height_px), (4, 8));
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        // The slow viewer's receiver sees the same Arc as the fast one: no
        // second encode.
        let n = encodes(sim_a).expect("hub");
        assert!(n >= 5 && n < 5 + 40, "{n} encodes for 5+ frames");
        drop(fast);
        drop(_slow);
        drop(hub_a);
        drop(hub_b);
        m.stop(&ws, &manager::Actor::Person { name: "owner".into() }, "test").expect("stop");
        crate::sim_routes::clear_lanes_for_tests();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn viewer_messages_map_to_point_actions_and_bad_ones_are_dropped() {
        assert!(matches!(to_action(&ViewerMsg::Tap { x: 1.0, y: 2.0 }), Some(Action::Tap { .. })));
        assert!(to_action(&ViewerMsg::OpenUrl { url: "no scheme".into() }).is_none());
        assert!(to_action(&ViewerMsg::Text { text: "x".repeat(5000) }).is_none());
        assert!(to_action(&ViewerMsg::Text { text: "x".repeat(TEXT_MAX_CHARS + 1) }).is_none());
        assert!(to_action(&ViewerMsg::Paste { text: "x".repeat(PASTE_MAX_BYTES + 1) }).is_none());
        assert!(to_action(&ViewerMsg::Ack { seq: 1 }).is_none());
    }

    /// A person's keyboard: typing is key events (no read-back), ⌘V is a
    /// paste, Shift/Option ride the key, ⌘A/⌘Z are edit chords.
    #[test]
    fn keyboard_messages_map_to_keyboard_actions() {
        use k2_core::sim::lane::{EditOp, Key};
        match to_action(&ViewerMsg::Text { text: "日本".into() }) {
            Some(Action::Keystrokes { text }) => assert_eq!(text.expose(), "日本"),
            other => panic!("text → keystrokes, got {other:?}"),
        }
        match to_action(&ViewerMsg::Paste { text: "a\nb".into() }) {
            Some(Action::Paste { text }) => assert_eq!(text.expose(), "a\nb"),
            other => panic!("paste → paste, got {other:?}"),
        }
        assert_eq!(
            to_action(&ViewerMsg::Key { key: Key::ArrowLeft, shift: true, alt: false }),
            Some(Action::Key { key: Key::ArrowLeft, mods: KeyMods { shift: true, alt: false } })
        );
        assert_eq!(to_action(&ViewerMsg::Edit { op: EditOp::Undo }), Some(Action::Edit { op: EditOp::Undo }));
        assert!(to_action(&ViewerMsg::Text { text: "a\u{1b}".into() }).is_none(), "control characters are refused");
    }

    /// Keyboard actions reach the device lane the same way touch does (the
    /// fake lane records them, never the text itself).
    #[test]
    fn keyboard_input_reaches_the_lane_through_person_input() {
        use k2_core::sim::lane::{EditOp, Key};
        let _serial = crate::sim_routes::sim_test_lock();
        let tmp = std::env::temp_dir().join(format!("k2-sim-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("ws")).expect("ws");
        k2_core::sim::set_base_for_tests(tmp.join("home"));
        k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
        let fake = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Android));
        k2_core::sim::register_lane(fake.clone());
        let m = manager::manager();
        m.refresh_probes();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'keys', ?2)", rusqlite::params![id, tmp.join("ws").display().to_string()])
                .expect("project");
        }
        let ws = Ws { id: id.clone(), handle: "keys".into(), path: tmp.join("ws") };
        m.boot(&ws, &manager::Actor::Person { name: "owner".into() }, &k2_core::sim::api::BootReq { no_tab: true, ..Default::default() }).expect("boot");
        let owner = Caller::Owner;
        person_input(&ws, "owner", &owner, ViewerMsg::Text { text: "hé".into() }).expect("text");
        person_input(&ws, "owner", &owner, ViewerMsg::Key { key: Key::ArrowRight, shift: true, alt: false }).expect("key");
        person_input(&ws, "owner", &owner, ViewerMsg::Edit { op: EditOp::SelectAll }).expect("edit");
        person_input(&ws, "owner", &owner, ViewerMsg::Paste { text: "pasted secret".into() }).expect("paste");
        let e = person_input(&ws, "owner", &owner, ViewerMsg::Paste { text: "x".repeat(PASTE_MAX_BYTES + 1) }).expect_err("too big");
        assert_eq!(e.code, "sim_invalid");
        let acts = fake.actions.lock().clone();
        let tail: Vec<&str> = acts.iter().rev().take(4).rev().map(String::as_str).collect();
        assert_eq!(tail, vec!["keys 2", "key ArrowRight+shift", "edit SelectAll", "paste 13"]);
        assert!(!acts.iter().any(|a| a.contains("secret")), "{acts:?}");
        m.stop(&ws, &manager::Actor::Person { name: "owner".into() }, "test").expect("stop");
        crate::sim_routes::clear_lanes_for_tests();
        let _ = std::fs::remove_dir_all(&tmp);
    }


    fn touch(phase: TouchPhase, x2: Option<f32>, y2: Option<f32>) -> ViewerMsg {
        ViewerMsg::Touch { phase, x: 10.0, y: 20.0, x2, y2, vx: None, vy: None }
    }

    #[test]
    fn a_second_finger_is_a_two_finger_touch_and_half_a_pair_is_dropped() {
        assert_eq!(
            to_action(&touch(TouchPhase::Move, Some(30.0), Some(40.0))),
            Some(Action::TwoTouch { phase: TouchPhase::Move, a: Point { x: 10.0, y: 20.0 }, b: Point { x: 30.0, y: 40.0 } })
        );
        assert_eq!(to_action(&touch(TouchPhase::Down, None, None)), Some(Action::Touch { phase: TouchPhase::Down, at: Point { x: 10.0, y: 20.0 } }));
        assert!(to_action(&touch(TouchPhase::Move, Some(30.0), None)).is_none(), "never one finger for a pinch");
    }

    /// The fake lane end to end: a flick's `up` with a velocity replays the
    /// tail of moves before lifting; a pinch reaches an Android lane as two
    /// fingers and is refused (not sent as one) on iOS; hello reports it.
    #[test]
    fn fling_tail_and_pinch_reach_the_fake_lane() {
        let _serial = crate::sim_routes::sim_test_lock();
        let tmp = std::env::temp_dir().join(format!("k2-sim-gesture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("ws")).expect("ws");
        k2_core::sim::set_base_for_tests(tmp.join("home"));
        k2_core::sim::limits::save_settings(&k2_core::sim::limits::SimSettings { min_free_ram_bytes: Some(0), ..Default::default() }).expect("limits");
        let fake = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Android));
        k2_core::sim::register_lane(fake.clone());
        let m = manager::manager();
        m.refresh_probes();
        let id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, 'gesture', ?2)", rusqlite::params![id, tmp.join("ws").display().to_string()])
                .expect("project");
        }
        let ws = Ws { id: id.clone(), handle: "gesture".into(), path: tmp.join("ws") };
        let owner = manager::Actor::Person { name: "owner".into() };
        m.boot(&ws, &owner, &k2_core::sim::api::BootReq { no_tab: true, ..Default::default() }).expect("boot");
        assert_eq!(m.input_caps(k2_core::sim::LaneKind::Android), InputCaps { multi_touch: true, fling: true });
        fake.actions.lock().clear();

        let fling = ViewerMsg::Touch { phase: TouchPhase::Up, x: 200.0, y: 600.0, x2: None, y2: None, vx: Some(0.0), vy: Some(-2000.0) };
        person_input(&ws, "owner", &Caller::Owner, fling).expect("fling");
        let got = fake.actions.lock().clone();
        assert_eq!(got.len(), 5, "{got:?}");
        assert!(got[..4].iter().all(|a| a.starts_with("touch Move 200,")), "{got:?}");
        assert!(got[4].starts_with("touch Up 200,53"), "lifts at the tail's end: {got:?}");

        fake.actions.lock().clear();
        let slow = ViewerMsg::Touch { phase: TouchPhase::Up, x: 200.0, y: 600.0, x2: None, y2: None, vx: Some(0.0), vy: Some(-5.0) };
        person_input(&ws, "owner", &Caller::Owner, slow).expect("slow");
        assert_eq!(fake.actions.lock().clone(), vec!["touch Up 200,600".to_string()]);

        fake.actions.lock().clear();
        let pinch = ViewerMsg::Touch { phase: TouchPhase::Down, x: 150.0, y: 400.0, x2: Some(250.0), y2: Some(400.0), vx: None, vy: None };
        person_input(&ws, "owner", &Caller::Owner, pinch).expect("pinch");
        assert_eq!(fake.actions.lock().clone(), vec!["two-touch Down 150,400 250,400".to_string()]);

        m.stop(&ws, &owner, "test").expect("stop");
        crate::sim_routes::clear_lanes_for_tests();

        // iOS takes one finger only: the pinch is refused, nothing is sent.
        let ios = Arc::new(k2_core::sim::fake::FakeLane::new(k2_core::sim::LaneKind::Ios));
        k2_core::sim::register_lane(ios.clone());
        m.refresh_probes();
        m.boot(&ws, &owner, &k2_core::sim::api::BootReq { platform: Some("ios".into()), no_tab: true, ..Default::default() }).expect("boot ios");
        assert!(!m.input_caps(k2_core::sim::LaneKind::Ios).multi_touch);
        ios.actions.lock().clear();
        let pinch = ViewerMsg::Touch { phase: TouchPhase::Down, x: 150.0, y: 400.0, x2: Some(250.0), y2: Some(400.0), vx: None, vy: None };
        let err = person_input(&ws, "owner", &Caller::Owner, pinch).expect_err("iOS refuses a pinch");
        assert!(err.message.contains("Pinch isn't available"), "{}", err.message);
        assert!(ios.actions.lock().is_empty(), "nothing reached the device");
        m.stop(&ws, &owner, "test").expect("stop ios");
        crate::sim_routes::clear_lanes_for_tests();
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
