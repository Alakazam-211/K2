//! The voice media path's lane-down state machine (voice rooms PRD v1.9
//! §6.6 M11/M12, V8; 0.46.8). Tunnel tier only.
//!
//! Signalling rides the main tunnel; TURN rides the voice lane
//! (`frpc-voice`). When the lane drops, sew-media's peers fall into a full
//! reconnect that keeps a participant for 20 s (`reconnect_window_ms`), so:
//!
//! | K2 state | When | K2 does |
//! |---|---|---|
//! | `up` | frpc-voice's turn proxy is registered | nothing; tells Sew `up` at once if it had said `down` |
//! | `voice_lane_reconnecting` | lane down for under 20 s | holds joins in its BFF ([`await_join`], at most 20 s each); tells Sew nothing |
//! | `voice_lane_down` | lane down for 20 s straight | `POST /mgmt/v1/voice/media-path {"state":"down","reason":"voice_lane_down"}`; refuses joins itself |
//! | `off` | not the tunnel tier, or the preview is off | nothing; a move to direct tells Sew `up` if it had said `down` |
//!
//! - **Up** is sent at once on frpc-voice's first `start proxy success` (the
//!   lane's listener wakes this loop): no hysteresis; the 20 s rule caps
//!   flapping at one `down` per 20 s.
//! - **Re-assert:** Sew keeps the state in memory and starts `up`. K2 sends
//!   `down` again after any Sew start it does ([`sew_started`]), compares
//!   `status.voice.media_path.state` with the lane every 10 s while down,
//!   and corrects Sew once at daemon boot (both ways).
//! - **Gated:** the route is called only when Sew's FEATURES list
//!   `voice_media_path`; without it K2 still holds and refuses joins.
//! - Not through the S27 outbox (this is live state): retried with backoff
//!   (1 s doubling, at most 30 s) on any failure; the call is idempotent.
//! - The loop also arms the voice lane exactly while voice runs the tunnel
//!   tier with the preview on, and stops it otherwise.
//!
//! **For lane C's BFF** (`/cli/voice/join`): call [`await_join`] (async) or
//! [`join_gate`] before letting a join through. `Ok` = go; `Err` = refuse
//! with [`JoinRefused::code`] (`media_path_down`) and its reason.

use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::ops::MgmtError;
use super::supervisor::{self as sup, Env};
use super::voice::{self, TIER_TUNNEL};

/// Lane down this long → `voice_lane_down` (§6.6 M12: Sew's
/// `reconnect_window_ms`).
pub const GRACE: Duration = Duration::from_secs(20);
/// While down, compare Sew's state with the lane this often.
pub const REASSERT_EVERY: Duration = Duration::from_secs(10);
/// A held join waits at most this long.
pub const JOIN_HOLD_MAX: Duration = GRACE;
/// Sew's FEATURES flag for the route (SW-11).
pub const FEATURE_VOICE_MEDIA_PATH: &str = "voice_media_path";
/// SW-11's route.
pub const ROUTE: &str = "/mgmt/v1/voice/media-path";
/// The reason K2 gives Sew (`details.media_path_reason`).
pub const REASON_LANE_DOWN: &str = "voice_lane_down";
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// K2's view of the media path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathState {
    /// Not the tunnel tier (or the preview is off): nothing to watch.
    Off,
    Up,
    /// Lane down under [`GRACE`]: joins held, Sew told nothing.
    Reconnecting,
    /// Lane down [`GRACE`] or more: Sew told `down`, joins refused.
    Down,
}

impl PathState {
    pub fn as_str(self) -> &'static str {
        match self {
            PathState::Off => "off",
            PathState::Up => "up",
            PathState::Reconnecting => "voice_lane_reconnecting",
            PathState::Down => "voice_lane_down",
        }
    }
}

/// What Sew holds (`up` / `down`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SewSide {
    Up,
    Down,
}

impl SewSide {
    fn as_str(self) -> &'static str {
        match self {
            SewSide::Up => "up",
            SewSide::Down => "down",
        }
    }
}

/// What the loop does with Sew next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Send(SewSide),
    /// Read `status.voice.media_path.state` (boot correction, re-assert).
    ReadSew,
}

/// The pure machine (time injected; no I/O).
#[derive(Debug, Clone)]
pub struct Machine {
    state: PathState,
    since: Instant,
    /// What Sew holds as far as K2 knows (`None` = unknown: boot).
    acked: Option<SewSide>,
    next_try: Option<Instant>,
    backoff: Duration,
    last_read: Option<Instant>,
}

impl Machine {
    pub fn new(now: Instant) -> Self {
        Self { state: PathState::Off, since: now, acked: None, next_try: None, backoff: BACKOFF_FIRST, last_read: None }
    }

    pub fn state(&self) -> PathState {
        self.state
    }

    pub fn since(&self) -> Instant {
        self.since
    }

    pub fn acked(&self) -> Option<SewSide> {
        self.acked
    }

    fn set(&mut self, s: PathState, now: Instant) {
        if self.state != s {
            self.state = s;
            self.since = now;
        }
    }

    /// One look at the world: `active` = the tunnel tier with the preview
    /// on; `lane_up` = frps holds the voice lane's turn proxy.
    pub fn observe(&mut self, active: bool, lane_up: bool, now: Instant) {
        if !active {
            self.set(PathState::Off, now);
        } else if lane_up {
            self.set(PathState::Up, now);
        } else {
            match self.state {
                PathState::Off | PathState::Up => self.set(PathState::Reconnecting, now),
                PathState::Reconnecting if now.duration_since(self.since) >= GRACE => self.set(PathState::Down, now),
                PathState::Reconnecting | PathState::Down => {}
            }
        }
    }

    /// What Sew should hold.
    pub fn want(&self) -> SewSide {
        if self.state == PathState::Down {
            SewSide::Down
        } else {
            SewSide::Up
        }
    }

    /// The next thing to do with Sew (`None` = nothing now).
    pub fn next_action(&mut self, now: Instant) -> Option<Action> {
        if self.next_try.is_some_and(|t| now < t) {
            return None;
        }
        let want = self.want();
        let read_due = self.last_read.is_none_or(|t| now.duration_since(t) >= REASSERT_EVERY);
        match self.acked {
            // Agreed. While down, compare with Sew every 10 s (a Sew
            // restart forgets `down`).
            Some(a) if a == want => (want == SewSide::Down && read_due).then(|| {
                self.last_read = Some(now);
                Action::ReadSew
            }),
            // Unknown and up: read once (a boot after a `down`).
            None if want == SewSide::Up => read_due.then(|| {
                self.last_read = Some(now);
                Action::ReadSew
            }),
            _ => Some(Action::Send(want)),
        }
    }

    /// A send finished.
    pub fn sent(&mut self, side: SewSide, ok: bool, now: Instant) {
        if ok {
            self.acked = Some(side);
            self.next_try = None;
            self.backoff = BACKOFF_FIRST;
            // Sew just answered: the next comparison is due in 10 s.
            self.last_read = Some(now);
        } else {
            self.fail(now);
        }
    }

    /// A read finished: Sew's state, or `None` when it couldn't be read.
    pub fn read(&mut self, sew: Option<SewSide>, now: Instant) {
        match sew {
            Some(s) => {
                self.acked = Some(s);
                self.next_try = None;
                self.backoff = BACKOFF_FIRST;
            }
            None => self.fail(now),
        }
    }

    fn fail(&mut self, now: Instant) {
        self.next_try = Some(now + self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }

    /// K2 (re)started Sew: it starts `up`.
    pub fn sew_started(&mut self) {
        self.acked = Some(SewSide::Up);
        self.next_try = None;
        self.backoff = BACKOFF_FIRST;
    }
}

// ── the published state (what the BFF and status read) ───────────────────

/// The media path now (for `/cli/sew/status`, `k2 tunnel status`, the BFF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub state: PathState,
    /// When `state` began (unix secs; 0 before the loop ran).
    pub since: i64,
    /// What Sew holds as far as K2 knows.
    pub sew: Option<SewSide>,
    /// Sew lists `voice_media_path` (else K2 only holds/refuses joins).
    pub sew_has_route: bool,
}

impl Snapshot {
    pub fn json(&self) -> Value {
        json!({
            "state": self.state.as_str(),
            "since": self.since,
            "sewState": self.sew.map(SewSide::as_str),
            "sewHasRoute": self.sew_has_route,
        })
    }
}

static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot { state: PathState::Off, since: 0, sew: None, sew_has_route: false });

/// The current media-path state (`Off` before the loop runs or off Linux).
pub fn snapshot() -> Snapshot {
    *SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner())
}

fn publish(m: &Machine, sew_has_route: bool, now: Instant) -> Snapshot {
    let since = unix_now() - now.saturating_duration_since(m.since()).as_secs() as i64;
    let new = Snapshot { state: m.state(), since, sew: m.acked(), sew_has_route };
    let mut s = SNAPSHOT.lock().unwrap_or_else(|p| p.into_inner());
    if s.state != new.state {
        k2_core::log_debug!("[sew] voice media path: {} -> {}", s.state.as_str(), new.state.as_str());
    }
    *s = new;
    new
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── the BFF's join gate (lane C) ─────────────────────────────────────────

/// A join refused because voice is unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinRefused {
    pub reason: &'static str,
}

impl JoinRefused {
    /// The error code the BFF answers (503, as Sew's `voice_unavailable`
    /// with `details.engine = media_path_down`).
    pub const CODE: &'static str = "media_path_down";
    pub fn code(&self) -> &'static str {
        Self::CODE
    }
}

/// What a join may do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinGate {
    Open,
    /// The lane is reconnecting: hold the join (at most [`JOIN_HOLD_MAX`]).
    Hold,
    Refuse,
}

/// The join gate for the current state (no waiting).
pub fn join_gate() -> JoinGate {
    gate_of(snapshot().state)
}

fn gate_of(s: PathState) -> JoinGate {
    match s {
        PathState::Off | PathState::Up => JoinGate::Open,
        PathState::Reconnecting => JoinGate::Hold,
        PathState::Down => JoinGate::Refuse,
    }
}

/// Hold a join while the lane reconnects (at most [`JOIN_HOLD_MAX`]), then
/// let it through (`Ok`) or refuse it. For lane C's `/cli/voice/join`.
pub async fn await_join() -> Result<(), JoinRefused> {
    let deadline = tokio::time::Instant::now() + JOIN_HOLD_MAX;
    loop {
        match join_gate() {
            JoinGate::Open => return Ok(()),
            JoinGate::Refuse => return Err(JoinRefused { reason: REASON_LANE_DOWN }),
            JoinGate::Hold if tokio::time::Instant::now() >= deadline => return Err(JoinRefused { reason: REASON_LANE_DOWN }),
            JoinGate::Hold => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

// ── the loop ─────────────────────────────────────────────────────────────

static WAKE: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();
static SEW_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn wake() {
    if let Some(tx) = WAKE.get() {
        let _ = tx.lock().unwrap_or_else(|p| p.into_inner()).send(());
    }
}

/// The voice lane went up or down (k2-core's listener).
fn on_lane_change(_up: bool) {
    wake();
}

/// K2 started or restarted `sew.service` (it starts `up`): re-send `down`
/// if the lane is down. Called from the supervisor's helper wrapper.
pub fn sew_started() {
    SEW_STARTED.store(true, std::sync::atomic::Ordering::SeqCst);
    wake();
}

/// Is the box's voice on the tunnel tier with the preview on?
pub fn active(env: &Env) -> bool {
    if !env.ops.tunnel_preview() {
        return false;
    }
    let st = voice::load(env);
    st.installed && voice::tier(&st) == TIER_TUNNEL
}

/// Sew's `status.voice.media_path.state`.
fn read_sew(env: &Env) -> Option<SewSide> {
    let st = sup::mgmt(env, "GET", "/mgmt/v1/status", None).ok()?;
    match st.pointer("/voice/media_path/state").and_then(Value::as_str) {
        Some("down") => Some(SewSide::Down),
        // Sew starts `up`; no field = up.
        _ => Some(SewSide::Up),
    }
}

fn send(env: &Env, side: SewSide) -> Result<(), MgmtError> {
    let body = match side {
        SewSide::Down => json!({"state": "down", "reason": REASON_LANE_DOWN}),
        SewSide::Up => json!({"state": "up"}),
    };
    sup::mgmt(env, "POST", ROUTE, Some(&body)).map(|_| ())
}

/// One pass: arm or stop the voice lane to match the tier, observe, and
/// act on Sew (only when `sew_has_route`). Returns the published snapshot.
pub fn drive(env: &Env, m: &mut Machine, sew_has_route: bool, now: Instant) -> Snapshot {
    let active = active(env);
    // The lane runs exactly while voice is on the tunnel tier.
    let armed = env.ops.voice_lane_armed();
    if active && !armed {
        if let Err(e) = env.ops.voice_lane(true) {
            k2_core::log_debug!("[sew] voice lane: {e}");
        }
    } else if !active && armed {
        let _ = env.ops.voice_lane(false);
    }
    if SEW_STARTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        m.sew_started();
    }
    m.observe(active, env.ops.voice_lane_up(), now);
    if sew_has_route {
        // At most a few steps a pass (a read can lead to a send).
        for _ in 0..3 {
            match m.next_action(now) {
                None => break,
                Some(Action::ReadSew) => {
                    let r = read_sew(env);
                    m.read(r, now);
                }
                Some(Action::Send(side)) => {
                    let r = send(env, side);
                    if let Err(e) = &r {
                        k2_core::log_debug!("[sew] voice media-path {}: {e} (retrying)", side.as_str());
                    } else {
                        k2_core::log_debug!("[sew] told Sew the voice media path is {}", side.as_str());
                    }
                    m.sent(side, r.is_ok(), now);
                }
            }
        }
    }
    publish(m, sew_has_route, now)
}

/// Start the loop (Linux, once): a pass every second and at once on a lane
/// change or a Sew start.
pub fn spawn() {
    if !super::sew_supported() || WAKE.get().is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel::<()>();
    if WAKE.set(Mutex::new(tx)).is_err() {
        return;
    }
    k2_core::tunnel::voice_lane::set_listener(on_lane_change);
    let _ = std::thread::Builder::new().name("sew-media-path".into()).spawn(move || {
        let env = sup::live();
        let mut m = Machine::new(Instant::now());
        let mut has_route = false;
        let mut route_checked: Option<Instant> = None;
        loop {
            let now = Instant::now();
            if route_checked.is_none_or(|t| now.duration_since(t) >= REASSERT_EVERY) {
                route_checked = Some(now);
                has_route = k2_core::tool_hook::sew_has(FEATURE_VOICE_MEDIA_PATH).unwrap_or(false);
            }
            drive(&env, &mut m, has_route, now);
            let _ = rx.recv_timeout(Duration::from_secs(1));
        }
    });
}

#[cfg(all(test, unix))]
#[path = "media_path_tests.rs"]
mod tests;
