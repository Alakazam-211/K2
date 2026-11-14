//! The follower (§14.2, §14.3): one task per held person, a Sew gateway
//! websocket on Sew's loopback listener through K2's shared gateway client
//! ([`crate::sew::gateway`], the kit feed's own, HP-37), unread kept in
//! [`super::state::PersonState`], frames for the person's clients on their
//! own broadcast channel. Clients never open their own Sew sockets: one
//! websocket per person, well under Sew's 10 per login.
//!
//! **The loop.** Sign in (or reuse the held session), take a gateway ticket
//! as the person, connect, read HELLO, IDENTIFY (or RESUME after a drop),
//! then follow: dispatches update the
//! state; the heartbeat runs at HELLO's interval; the renew timer swaps a
//! renewed token onto the live socket with op 11 REAUTH (HP-28); frames the
//! routes queue (typing, subscribe) go out in order.
//!
//! **A drop** (§14.3): RESUME with the gateway session and seq. When Sew
//! says the session can't resume (INVALID_SESSION), K2 catches up through
//! `GET /api/v1/me/changes?since=<member seq>` and the changed channels'
//! read states; on `410 resync_required` it reads `GET /api/v1/me/sync` and
//! rebuilds every read state ([`catch_up`]). Then it IDENTIFYs again.
//!
//! **Fan-out is generic**: besides the chat events, [`FORWARD`] events
//! (`call_ring`, `call_updated`) go to the person's clients as they came,
//! so DM calls can ride the same events socket later.
//!
//! **Close 4005** (`already_authenticated`: past Sew's per-login socket
//! cap) is a bug in K2 and is logged loudly.
//!
//! **The supervisor** (every [`TICK`]): Home chat on and Sew up, the people
//! with a live login session on this server are held; anyone else is
//! dropped and their held session revoked (HP-24). Nothing is on disk: a
//! daemon restart starts every follower again from READY.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::sew::gateway::{self, Started};

use super::sessions::{self, Session};
use super::state::PersonState;
use super::{deps, now_secs};
use crate::voice::bff::Refusal;

/// The supervisor's tick (a disabled login's session ends within one).
pub const TICK: Duration = Duration::from_secs(5);
pub use crate::sew::gateway::op;
/// Sew events passed to the person's clients as they came (`{"type":
/// "event", "t", "d"}`), beside the chat frames.
pub const FORWARD: &[&str] = &["call_ring", "call_updated"];
/// Sew's close code past its per-login socket cap.
pub const CLOSE_ALREADY_AUTHENTICATED: u16 = 4005;
/// Most catch-up pages read before a resync is cheaper.
const CATCH_UP_PAGES: usize = 20;

/// One held person.
pub struct Held {
    pub ext: String,
    pub state: Mutex<PersonState>,
    pub session: Mutex<Option<Session>>,
    /// One renew at a time for this person (Sew renews a session once).
    renewing: Mutex<()>,
    /// `connecting` | `live` | `unavailable`.
    pub status: Mutex<&'static str>,
    /// Frames for the person's client sockets.
    pub events: broadcast::Sender<String>,
    /// Frames to send on the gateway socket (typing, subscribe).
    out: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// How many gateway sockets this follower opened (tests read it).
    pub connects: AtomicU32,
    stop: Notify,
    stopped: AtomicBool,
}

impl Held {
    fn new(ext: &str) -> Held {
        Held {
            ext: ext.to_string(),
            state: Mutex::new(PersonState::default()),
            session: Mutex::new(None),
            renewing: Mutex::new(()),
            status: Mutex::new("connecting"),
            events: broadcast::channel(256).0,
            out: Mutex::new(None),
            connects: AtomicU32::new(0),
            stop: Notify::new(),
            stopped: AtomicBool::new(false),
        }
    }

    /// Publish `frames` to the person's client sockets.
    pub fn publish(&self, frames: &[Value]) {
        for f in frames {
            let _ = self.events.send(f.to_string());
        }
    }

    /// Queue a frame for the gateway socket (false: not connected).
    pub fn send_gateway(&self, op: u64, d: Value) -> bool {
        let g = self.out.lock().unwrap_or_else(|p| p.into_inner());
        g.as_ref().is_some_and(|tx| tx.send(json!({"op": op, "d": d}).to_string()).is_ok())
    }

    /// The held session's token, signing in first when none is held or it
    /// expired.
    pub fn token(&self) -> Result<String, Refusal> {
        let now = now_secs();
        {
            let g = self.session.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(s) = g.as_ref().filter(|s| !s.expired(now + 5)) {
                return Ok(s.token.clone());
            }
        }
        let s = sessions::sign_in(&self.ext, now)?;
        let t = s.token.clone();
        if let Ok(mut st) = self.state.lock() {
            if st.me.is_empty() {
                st.me = s.member_id.clone();
            }
        }
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
        Ok(t)
    }

    /// Renew the held session now (blocking) and switch to the new token at
    /// once; returns it. Single-flighted: a caller that waited while another
    /// renewed the same session gets that result and never renews it again.
    /// Any 401 from Sew's renew falls back to a fresh sign-in.
    pub fn renew_now(&self, now: i64) -> Result<String, Refusal> {
        let cur = self
            .session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| sessions::unavailable("no session to renew"))?;
        let _one = self.renewing.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = self.session.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            if s.token != cur.token {
                return Ok(s.token.clone());
            }
        }
        let s = match sessions::renew(&self.ext, &cur, now, &sessions::renew_key()) {
            Ok(s) => s,
            Err(r) if r.status == 401 => {
                k2_core::log_debug!("[home-chat] {}: renew answered 401 {}; signing in again", self.ext, r.code);
                sessions::sign_in(&self.ext, now)?
            }
            Err(r) => return Err(r),
        };
        let t = s.token.clone();
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(s);
        Ok(t)
    }

    /// Drop the held session (Sew answered 401 with it).
    pub fn forget_session(&self) {
        *self.session.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    fn set_status(&self, s: &'static str) {
        let mut g = self.status.lock().unwrap_or_else(|p| p.into_inner());
        if *g != s {
            *g = s;
            drop(g);
            self.publish(&[json!({"type": "status", "chat": s})]);
        }
    }

    pub fn status(&self) -> &'static str {
        *self.status.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn halt(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.stop.notify_waiters();
        self.stop.notify_one();
    }
}

fn held() -> &'static Mutex<HashMap<String, Arc<Held>>> {
    static H: OnceLock<Mutex<HashMap<String, Arc<Held>>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// The held person `ext`, if held.
pub fn get(ext: &str) -> Option<Arc<Held>> {
    held().lock().unwrap_or_else(|p| p.into_inner()).get(ext).cloned()
}

/// Hold `ext` (start its follower when it isn't running). Needs a tokio
/// runtime when it starts one.
pub fn hold(ext: &str) -> Arc<Held> {
    let mut g = held().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(h) = g.get(ext) {
        return h.clone();
    }
    let h = Arc::new(Held::new(ext));
    g.insert(ext.to_string(), h.clone());
    drop(g);
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let h2 = h.clone();
        rt.spawn(async move { run(h2).await });
    }
    h
}

/// Stop following `ext` and end its held session in Sew.
pub fn release(ext: &str) {
    let h = held().lock().unwrap_or_else(|p| p.into_inner()).remove(ext);
    if let Some(h) = h {
        h.halt();
        h.publish(&[json!({"type": "status", "chat": "ended"})]);
        sessions::revoke(ext);
        k2_core::log_debug!("[home-chat] {ext}: held session dropped and revoked");
    }
}

/// Every held person (`k2 voice status`-style views, tests).
pub fn held_people() -> Vec<String> {
    let mut v: Vec<String> = held().lock().unwrap_or_else(|p| p.into_inner()).keys().cloned().collect();
    v.sort();
    v
}

/// Drop everyone (Home chat turned off, tests).
pub fn release_all() {
    for ext in held_people() {
        release(&ext);
    }
}

// ── the supervisor ───────────────────────────────────────────────────

/// Who should be held now: the box people with a live login session.
pub fn wanted_people() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(live) = k2_core::connect_users::live_session_usernames() else { return out };
    let linked = k2_core::app_settings::load().owner_accounts;
    let Ok(users) = k2_core::connect_users::admin_space_facts() else { return out };
    for (id, username, _role, disabled) in users {
        if disabled || !live.contains(&username) {
            continue;
        }
        if linked.iter().any(|l| l.eq_ignore_ascii_case(&username)) {
            out.insert("k2:owner".to_string());
        } else if !id.is_empty() && matches!(k2_core::tool_hook::connect_chat_gate(&id, &username), Ok(k2_core::tool_hook::ConnectChatGate::Ready)) {
            out.insert(format!("k2:connect:{id}"));
        }
    }
    out
}

/// One supervisor pass: hold the wanted, release the rest.
pub fn reconcile(wanted: &BTreeSet<String>) {
    for ext in held_people() {
        if !wanted.contains(&ext) {
            release(&ext);
        }
    }
    for ext in wanted {
        hold(ext);
    }
}

/// Start the supervisor (daemon boot). Idempotent.
pub fn spawn() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        let mut tick = tokio::time::interval(TICK);
        loop {
            tick.tick().await;
            let on = super::status().0;
            let wanted = if on { tokio::task::spawn_blocking(wanted_people).await.unwrap_or_default() } else { BTreeSet::new() };
            reconcile(&wanted);
        }
    });
}

// ── catch-up (§14.3) ─────────────────────────────────────────────────

/// What a catch-up did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatchUp {
    /// The changes feed: these channels' read states were read again.
    Changes { channels: usize },
    /// `410 resync_required`: every read state rebuilt from `me/sync`.
    Resync,
}

/// Catch `st` up from Sew over HTTP with `token` (blocking).
pub fn catch_up(token: &str, st: &mut PersonState) -> Result<CatchUp, Refusal> {
    let d = deps();
    let mut since = st.member_seq;
    let mut changed: BTreeSet<String> = BTreeSet::new();
    for _ in 0..CATCH_UP_PAGES {
        let a = d.sew.get(Some(token), &format!("/api/v1/me/changes?since={since}")).map_err(sessions::unavailable)?;
        if a.status == 410 {
            let s = d.sew.get(Some(token), "/api/v1/me/sync").map_err(sessions::unavailable)?;
            if !s.ok() {
                return Err(crate::voice::bff::sew_refusal(&s));
            }
            st.apply_sync(&s.body);
            return Ok(CatchUp::Resync);
        }
        if !a.ok() {
            return Err(crate::voice::bff::sew_refusal(&a));
        }
        for e in a.body["entries"].as_array().into_iter().flatten() {
            match &e["entity_id"] {
                Value::String(s) => {
                    changed.insert(s.clone());
                }
                Value::Number(n) => {
                    changed.insert(n.to_string());
                }
                _ => {}
            }
        }
        since = a.body["next_since"].as_u64().unwrap_or(since);
        if !a.body["has_more"].as_bool().unwrap_or(false) {
            break;
        }
    }
    let ids: Vec<String> = changed.into_iter().collect();
    for chunk in ids.chunks(100) {
        let a = d.sew.get(Some(token), &format!("/api/v1/me/read_states?channel_ids={}", chunk.join(","))).map_err(sessions::unavailable)?;
        if !a.ok() {
            return Err(crate::voice::bff::sew_refusal(&a));
        }
        let list = a.body.get("read_states").unwrap_or(&a.body);
        for rs in list.as_array().into_iter().flatten() {
            st.apply_read_state(rs);
        }
    }
    st.member_seq = since;
    Ok(CatchUp::Changes { channels: ids.len() })
}

// ── the gateway loop ─────────────────────────────────────────────────

fn frame(op: u64, d: Value) -> Message {
    Message::Text(json!({"op": op, "d": d}).to_string().into())
}

fn flag_unread_count() -> bool {
    (deps().flags)().read_state_unread_count
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

/// Why one socket ended.
enum Ended {
    Stop,
    /// Reconnect; `resume` keeps the gateway session.
    Again { resume: bool },
}

async fn run(h: Arc<Held>) {
    let mut backoff = Duration::from_secs(1);
    let mut resume = false;
    while !h.stopped.load(Ordering::SeqCst) {
        let h2 = h.clone();
        let token = match blocking(move || h2.token()).await {
            Some(Ok(t)) => t,
            Some(Err(r)) => {
                k2_core::log_debug!("[home-chat] {}: no session ({} {})", h.ext, r.code, r.hint);
                h.set_status("unavailable");
                if wait_or_stop(&h, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
            None => return,
        };
        match connect_once(&h, &token, resume).await {
            Ended::Stop => return,
            Ended::Again { resume: r } => {
                resume = r;
                if !r {
                    // The gateway session is gone: catch up over HTTP first.
                    let h2 = h.clone();
                    let tok = token.clone();
                    let _ = blocking(move || {
                        let mut st = h2.state.lock().unwrap_or_else(|p| p.into_inner());
                        if st.member_seq > 0 {
                            match catch_up(&tok, &mut st) {
                                Ok(c) => k2_core::log_debug!("[home-chat] {}: caught up ({c:?})", h2.ext),
                                Err(e) => k2_core::log_debug!("[home-chat] {}: catch-up failed: {} {}", h2.ext, e.code, e.hint),
                            }
                        }
                    })
                    .await;
                }
            }
        }
        h.set_status("connecting");
        if wait_or_stop(&h, backoff).await {
            return;
        }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

/// Sleep `d`, or return true at once when the follower is stopped.
async fn wait_or_stop(h: &Held, d: Duration) -> bool {
    if h.stopped.load(Ordering::SeqCst) {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(d) => h.stopped.load(Ordering::SeqCst),
        _ = h.stop.notified() => true,
    }
}

/// A single-use gateway ticket as the person (blocking).
fn ticket(token: &str) -> Result<String, Refusal> {
    let a = deps().sew.post(Some(token), gateway::TICKET, &json!({})).map_err(sessions::unavailable)?;
    if !a.ok() {
        return Err(crate::voice::bff::sew_refusal(&a));
    }
    a.body["ticket"].as_str().filter(|t| !t.is_empty()).map(str::to_string).ok_or_else(|| sessions::unavailable("Sew's ticket answer had no ticket"))
}

/// Sew's loopback gateway address (never a public one).
fn gateway_addr() -> Option<std::net::SocketAddr> {
    let sa: std::net::SocketAddr = (deps().gateway_addr)().parse().ok()?;
    sa.ip().is_loopback().then_some(sa)
}

async fn connect_once(h: &Arc<Held>, token: &str, resume: bool) -> Ended {
    let Some(sa) = gateway_addr() else {
        h.set_status("unavailable");
        return Ended::Again { resume };
    };
    let tok = token.to_string();
    let t = match blocking(move || ticket(&tok)).await {
        Some(Ok(t)) => t,
        Some(Err(r)) => {
            if r.status == 401 {
                h.forget_session();
            }
            h.set_status("unavailable");
            return Ended::Again { resume };
        }
        None => return Ended::Stop,
    };
    let sock = match tokio::time::timeout(Duration::from_secs(10), gateway::connect(sa)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            k2_core::log_debug!("[home-chat] {}: gateway connect: {e:?}", h.ext);
            h.set_status("unavailable");
            return Ended::Again { resume };
        }
        Err(_) => {
            h.set_status("unavailable");
            return Ended::Again { resume };
        }
    };
    h.connects.fetch_add(1, Ordering::SeqCst);
    let gateway::Socket { mut ws, beat } = sock;
    let (gw_session, seq) = {
        let st = h.state.lock().unwrap_or_else(|p| p.into_inner());
        (st.gateway_session.clone(), st.seq)
    };
    let first = match (resume, gw_session) {
        (true, Some(sid)) => gateway::resume(&t, &sid, seq),
        _ => gateway::identify(&t, "k2-home-chat"),
    };
    if gateway::send(&mut ws, &first).await.is_err() {
        return Ended::Again { resume };
    }
    let unread_flag = {
        let f = flag_unread_count;
        blocking(f).await.unwrap_or(false)
    };
    match tokio::time::timeout(Duration::from_secs(15), gateway::started(&mut ws)).await {
        Ok(Some(Started::Ready { d, s })) => {
            let mut st = h.state.lock().unwrap_or_else(|p| p.into_inner());
            st.apply_ready(&d);
            st.seq = s.unwrap_or(0);
        }
        Ok(Some(Started::Resumed)) => {}
        Ok(Some(Started::Invalid { resumable })) => return Ended::Again { resume: resumable },
        Ok(None) | Err(_) => return Ended::Again { resume },
    }
    h.set_status("live");
    h.publish(&[json!({"type": "summary_changed"})]);
    let (mut write, mut read) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    *h.out.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
    let mut heartbeat = tokio::time::interval(beat);
    heartbeat.tick().await;
    let mut renew_check = tokio::time::interval(renew_every());
    renew_check.tick().await;
    let mut renew_after = 0i64;
    let ended = loop {
        tokio::select! {
            _ = h.stop.notified() => {
                let _ = write.send(Message::Close(None)).await;
                break Ended::Stop;
            }
            _ = heartbeat.tick() => {
                let s = h.state.lock().unwrap_or_else(|p| p.into_inner()).seq;
                if write.send(frame(op::HEARTBEAT, json!(s))).await.is_err() {
                    break Ended::Again { resume: true };
                }
            }
            _ = renew_check.tick() => {
                let now = now_secs();
                let cur = h.session.lock().unwrap_or_else(|p| p.into_inner()).clone();
                let Some(cur) = cur else { continue };
                if cur.expired(now) {
                    // Renewing never worked before expiry: sign in again.
                    h.forget_session();
                    break Ended::Again { resume: false };
                }
                if !cur.renew_due(now) || now < renew_after {
                    continue;
                }
                let h2 = h.clone();
                match blocking(move || h2.renew_now(now)).await {
                    Some(Ok(t)) => {
                        // HP-28: the socket keeps going on the renewed login.
                        if write.send(Message::Text(gateway::reauth(&t).to_string())).await.is_err() {
                            break Ended::Again { resume: true };
                        }
                        k2_core::log_debug!("[home-chat] {}: session renewed on the live socket", h.ext);
                    }
                    Some(Err(e)) => {
                        k2_core::log_debug!("[home-chat] {}: renew failed ({} {}); retrying", h.ext, e.code, e.hint);
                        renew_after = now + 30;
                    }
                    None => break Ended::Stop,
                }
            }
            out = rx.recv() => {
                if let Some(text) = out {
                    if write.send(Message::Text(text)).await.is_err() {
                        break Ended::Again { resume: true };
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                        match v["op"].as_u64() {
                            Some(op::DISPATCH) => {
                                let ev = v["t"].as_str().unwrap_or("");
                                let frames = {
                                    let mut st = h.state.lock().unwrap_or_else(|p| p.into_inner());
                                    if let Some(s) = v["s"].as_u64() {
                                        st.seq = s;
                                    }
                                    match ev {
                                        "ready" => {
                                            st.apply_ready(&v["d"]);
                                            vec![json!({"type": "summary_changed"})]
                                        }
                                        e if FORWARD.contains(&e) => vec![json!({"type": "event", "t": e, "d": v["d"]})],
                                        _ => st.apply_event(ev, &v["d"], unread_flag),
                                    }
                                };
                                h.publish(&frames);
                            }
                            Some(op::INVALID_SESSION) => {
                                let resumable = v["d"]["resumable"].as_bool().unwrap_or(false);
                                break Ended::Again { resume: resumable };
                            }
                            Some(op::RECONNECT) => break Ended::Again { resume: true },
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = write.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Close(c))) => {
                        if c.as_ref().is_some_and(|c| u16::from(c.code) == CLOSE_ALREADY_AUTHENTICATED) {
                            k2_core::log_debug!(
                                "[home-chat] BUG {}: Sew closed the follower with 4005 already_authenticated (more than its per-login socket cap); K2 holds one socket per person",
                                h.ext
                            );
                        }
                        break Ended::Again { resume: true };
                    }
                    Some(Ok(_)) => {}
                    None | Some(Err(_)) => break Ended::Again { resume: true },
                }
            }
        }
    };
    *h.out.lock().unwrap_or_else(|p| p.into_inner()) = None;
    ended
}

/// How often the renew timer looks (tests shorten it).
fn renew_every() -> Duration {
    Duration::from_millis(RENEW_EVERY_MS.load(Ordering::Relaxed))
}

static RENEW_EVERY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(15_000);

/// Tests: look for a due renew every `ms`.
#[doc(hidden)]
pub fn set_renew_every_for_tests(ms: u64) {
    RENEW_EVERY_MS.store(ms, Ordering::Relaxed);
}
