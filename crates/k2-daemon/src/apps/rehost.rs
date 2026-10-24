//! Re-host every App on a K2 update (prd-app-runtime-update-continuity-v1
//! S2, 0.46.4; Rosson 2026-10-24: "re-host the app (like how Vercel does
//! it) on the new version").
//!
//! After the engine heal reaches `ok`, one App at a time goes through the
//! existing deploy pipeline with kind `rehost` and its **live Seed**: a
//! fresh instance on the new engine, the health gate, the atomic switch, a
//! drain, the old instance warm for the watch window (auto-rollback to it).
//! People who refresh land on the new instance; nobody sees an outage.
//!
//! - **Due (R10):** the live deployment's fingerprint (`runtime_json`, from
//!   the `version` its `prepare` checked) names another K2 or engine
//!   version, or there is none (rows from before 0221).
//! - **Pre-flights:** 0 the hand-off's predecessor has exited (R36's
//!   re-binds done); 1 core upgrades (a root sequence, Sew, a Postgres
//!   move) are idle, waited for up to 30 min (R29, Q11); per App:
//!   `requires` (R30), sessions in memory → defer and ask (R31, Q9), and
//!   "can't run two copies" (extra ports, the process exception, policy or
//!   manifest `handover`, or no measured memory headroom, R12/Q15) → the
//!   App's toggle: `pause` (default: stop, then start) or `defer` (skip,
//!   the owner updates it; Rosson Q4 + Q16).
//! - **Order (R11, Q7):** fewest active connections first; 10 s between
//!   Apps; busy Apps are retried later.
//! - **Breaker (R11, A13):** two failures in a row (a failed gate or an
//!   auto-rollback) stop the wave and file one owner notice. A failed App is
//!   `rehost_failed` until the next K2 version or `k2 app rehost`.
//! - **Quiet (A13):** no `deploy_failed` wake, stall alert or rollback alert
//!   for a re-host (the supervisor skips them); this module owns the notes
//!   (R17): the agent hears a deferral older than an hour and a
//!   `rehost_failed`; the owner hears the breaker and the cards.
//! - **Policy (R14):** `auto` | `handover` | `window` | `manual` | `off`
//!   per App; `K2_APPS_REHOST=off` box-wide.
//!
//! Status: `GET /cli/app/rehost` (Member), `deploy_audit` events
//! (`rehost_due`, `rehost_deferred`, `rehost_failed`, `rehost_done`),
//! `k2 app history` (`rehost` rows), `k2 app rehost status`.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use k2_core::deploy::rehost::{self as rh, defer, PolicyGate, SessionStore};
use k2_core::deploy::store::{self, AppRow};
use k2_core::deploy::{audit as daudit, codes};
use serde::Serialize;
use serde_json::{json, Value as J};

use super::routes::{app_for, json_body, need_build, need_credential, no, s, Who, R};

/// Pause between Apps (R11).
pub const GAP: Duration = Duration::from_secs(10);
/// The longest the wave waits for core upgrades (R29, Q11).
pub const CORE_WAIT: Duration = Duration::from_secs(30 * 60);
/// Failures in a row that stop the wave (R11).
pub const BREAKER: u32 = 2;
/// Deferred Apps are retried this often.
pub const RETRY_EVERY: Duration = Duration::from_secs(3600);
/// The agent hears of a deferral older than this (R17).
pub const TELL_AGENT_AFTER: Duration = Duration::from_secs(3600);

// ── status ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    /// `due` | `running` | `done` | `deferred` | `failed` | `current`.
    pub state: String,
    pub code: Option<String>,
    pub detail: Option<String>,
    pub deployment: Option<String>,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WaveStatus {
    /// `idle` | `waiting` | `running` | `done` | `stopped`.
    pub state: String,
    pub reason: Option<String>,
    pub k2_version: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub apps: BTreeMap<String, AppStatus>,
}

fn status() -> &'static Mutex<WaveStatus> {
    static S: OnceLock<Mutex<WaveStatus>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(WaveStatus { state: "idle".into(), k2_version: env!("CARGO_PKG_VERSION").into(), ..Default::default() })
    })
}

fn with_status<R>(f: impl FnOnce(&mut WaveStatus) -> R) -> R {
    f(&mut status().lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn status_json() -> J {
    serde_json::to_value(with_status(|s| s.clone())).unwrap_or(J::Null)
}

fn now() -> i64 {
    k2_core::deploy::now()
}

fn set_app(app: &str, state: &str, code: Option<&str>, detail: Option<String>, deployment: Option<&str>) {
    with_status(|s| {
        s.apps.insert(
            app.to_string(),
            AppStatus { state: state.into(), code: code.map(Into::into), detail, deployment: deployment.map(Into::into), at: now() },
        );
    });
}

fn set_wave(state: &str, reason: Option<String>) {
    with_status(|s| {
        s.state = state.into();
        s.reason = reason;
        match state {
            "running" | "waiting" if s.started_at.is_none() => s.started_at = Some(now()),
            "done" | "stopped" => s.finished_at = Some(now()),
            _ => {}
        }
    });
}

// ── what the wave reads and does ─────────────────────────────────────

/// One re-host's end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Live on the new instance (`deployment`).
    Live(String),
    /// Never went live (`deployment`, why): the old instance keeps serving.
    Failed(String, String),
    /// `begin` refused it (a deploy in flight): retried later.
    Refused(String),
}

/// Everything the wave touches, so tests drive it with a fake box.
pub trait WaveOps {
    fn apps(&self) -> Vec<AppRow>;
    fn k2_version(&self) -> String;
    /// The running engine's version (`None`: no engine on this box).
    fn engine_version(&self) -> Option<String>;
    /// The engine heal is done and the engine answers with our protocol
    /// (or this box has no engine).
    fn engine_ready(&self) -> bool;
    /// R36 pre-flight 0: a hand-off predecessor still drains.
    fn predecessor_draining(&self) -> bool;
    /// R29: a core upgrade (and what) is running now.
    fn core_busy(&self) -> Option<String>;
    fn minute_of_day(&self) -> u32;
    fn rehost_off(&self) -> bool;
    fn sleep(&self, d: Duration);
    fn live_fingerprint(&self, app: &AppRow) -> Option<rh::Fingerprint>;
    fn runs_unwalled(&self, app: &AppRow) -> bool;
    fn busy(&self, app: &AppRow) -> bool;
    fn active_connections(&self, app: &AppRow) -> usize;
    /// R30: `Err(message)` when the live Seed's `requires` aren't met now.
    fn requires(&self, app: &AppRow) -> Result<(), String>;
    /// R31: where the live Seed keeps sessions.
    fn sessions(&self, app: &AppRow) -> SessionStore;
    /// Why two copies can't run (`needs_handover`), if they can't by shape.
    fn no_overlap_shape(&self, app: &AppRow) -> Option<String>;
    /// R12: `Err(words)` when a second instance doesn't fit by measure.
    fn headroom(&self, app: &AppRow) -> Result<(), String>;
    /// The App failed a re-host on this K2 version (held, A13).
    fn held_failed(&self, app: &AppRow) -> bool;
    /// Begin a re-host (`pause`: stop first) and wait for its end.
    fn rehost(&self, app: &AppRow, pause: bool, reason: &str) -> Outcome;
    fn audit(&self, app: &str, event: &str, payload: J);
    fn owner_notice(&self, title: &str, body: &str);
    fn agent_note(&self, app: &AppRow, text: &str);
}

/// How the owner (or an agent with switch 1) asked for one App now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Forced {
    /// Re-host even when everyone signs in again (sessions in memory).
    pub yes: bool,
    /// Stop first when it can't run two copies, even with `defer`.
    pub pause: bool,
}

/// What one App's turn decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Turn {
    Done(String),
    Failed(String, String),
    Deferred(&'static str, String),
    NotDue,
}

fn first_seen() -> &'static Mutex<HashMap<String, Instant>> {
    static M: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn told() -> &'static Mutex<std::collections::HashSet<(String, String)>> {
    static M: OnceLock<Mutex<std::collections::HashSet<(String, String)>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

/// Once per App per key (this process).
fn once(app: &str, key: &str) -> bool {
    told().lock().unwrap_or_else(|e| e.into_inner()).insert((app.to_string(), key.to_string()))
}

fn defer_app(ops: &dyn WaveOps, app: &AppRow, code: &'static str, detail: String) -> Turn {
    set_app(&app.app_id, "deferred", Some(code), Some(detail.clone()), None);
    ops.audit(&app.app_id, "rehost_deferred", json!({ "code": code, "detail": detail, "k2Version": ops.k2_version() }));
    // The cards: what the owner can do about it, once per App and reason.
    let card = match code {
        defer::SESSIONS_IN_MEMORY => Some((
            format!("App {} waits for you to update it", app.app_id),
            format!(
                "K2 updated, and the App {} keeps its sign-ins in memory: moving it to a fresh instance signs everyone \
                 out. Choose one: re-host now (everyone signs in again): k2 app rehost {} --yes ; or deploy the newer \
                 template first (its sessions survive); or leave it on the current instance (it keeps running).",
                app.app_id, app.app_id
            ),
        )),
        defer::NEEDS_HANDOVER | defer::NO_HEADROOM => Some((
            format!("App {} waits for you to update it", app.app_id),
            format!(
                "K2 updated, and the App {} can't run two copies at once ({detail}). Its setting is \"skip and let me \
                 update it myself\". Choose one: update now with a short pause: k2 app rehost {} --pause ; or leave it on \
                 the current instance (it keeps running). The App panel's update setting changes the default.",
                app.app_id, app.app_id
            ),
        )),
        _ => None,
    };
    if let Some((title, body)) = card {
        if once(&app.app_id, &format!("card:{code}:{}", ops.k2_version())) {
            ops.owner_notice(&title, &body);
        }
    }
    // R17: the agent hears a deferral that lasted an hour.
    let since = *first_seen().lock().unwrap_or_else(|e| e.into_inner()).entry(app.app_id.clone()).or_insert_with(Instant::now);
    if since.elapsed() >= TELL_AGENT_AFTER && once(&app.app_id, &format!("agent:{code}")) {
        ops.agent_note(
            app,
            &format!(
                "[k2 apps] App {}: K2 updated to {} but hasn't moved the App to the new runtime for over an hour ({code}: {detail}). \
                 It keeps running on its current instance. k2 app rehost status {}",
                app.app_id,
                ops.k2_version(),
                app.app_id
            ),
        );
    }
    Turn::Deferred(code, detail)
}

/// Is the App due (and not held), and why.
fn due(ops: &dyn WaveOps, app: &AppRow) -> Option<&'static str> {
    if app.live_seed.is_none() || app.state != store::STATE_RUNNING || ops.runs_unwalled(app) {
        return None;
    }
    rh::due(ops.live_fingerprint(app).as_ref(), &ops.k2_version(), ops.engine_version().as_deref())
}

/// One App's turn. `forced`: `k2 app rehost` (no policy, no hold).
pub fn turn(ops: &dyn WaveOps, app: &AppRow, forced: Option<Forced>) -> Turn {
    let why = match due(ops, app) {
        Some(w) => w,
        None if forced.is_some() && app.live_seed.is_some() => "requested",
        None => {
            set_app(&app.app_id, "current", None, None, None);
            return Turn::NotDue;
        }
    };
    if forced.is_none() {
        if ops.held_failed(app) {
            return defer_app(ops, app, defer::REHOST_FAILED, format!("its re-host on K2 {} failed; k2 app rehost {} tries again", ops.k2_version(), app.app_id));
        }
        if ops.rehost_off() {
            return defer_app(ops, app, defer::POLICY_OFF, format!("{} is off on this box", rh::REHOST_ENV));
        }
        if let PolicyGate::Wait(code) = rh::policy_gate(&app.rehost_policy, app.rehost_window.as_deref(), ops.minute_of_day()) {
            return defer_app(ops, app, code, format!("the App's re-host policy is {}", app.rehost_policy));
        }
    }
    if ops.busy(app) {
        return defer_app(ops, app, defer::BUSY, "a deploy of it is in progress".into());
    }
    if let Err(m) = ops.requires(app) {
        return defer_app(ops, app, defer::REQUIRES_UNMET, m);
    }
    if ops.sessions(app) == SessionStore::Memory && !forced.is_some_and(|f| f.yes) {
        return defer_app(ops, app, defer::SESSIONS_IN_MEMORY, "its sessions live in memory: a new instance signs everyone out".into());
    }
    // Two copies at once? By shape (handover), else by measured memory.
    let no_overlap = ops
        .no_overlap_shape(app)
        .map(|w| (defer::NEEDS_HANDOVER, w))
        .or_else(|| ops.headroom(app).err().map(|w| (defer::NO_HEADROOM, w)));
    let pause = match no_overlap {
        None => false,
        Some((code, w)) => {
            let defer_it = app.rehost_when_no_overlap == rh::NO_OVERLAP_DEFER && !forced.is_some_and(|f| f.pause);
            if defer_it {
                return defer_app(ops, app, code, w);
            }
            ops.audit(&app.app_id, "rehost_pause", json!({ "code": code, "detail": w }));
            true
        }
    };
    ops.audit(&app.app_id, "rehost_due", json!({ "why": why, "pause": pause, "k2Version": ops.k2_version() }));
    set_app(&app.app_id, "running", None, Some(format!("due: {why}{}", if pause { "; a short pause" } else { "" })), None);
    let reason = format!("K2 updated to {}", ops.k2_version());
    match ops.rehost(app, pause, &reason) {
        Outcome::Live(dep) => {
            set_app(&app.app_id, "done", None, None, Some(&dep));
            ops.audit(&app.app_id, "rehost_done", json!({ "deployment": dep, "k2Version": ops.k2_version() }));
            first_seen().lock().unwrap_or_else(|e| e.into_inner()).remove(&app.app_id);
            Turn::Done(dep)
        }
        Outcome::Failed(dep, w) => {
            mark_failed(ops, app, &dep, &w);
            Turn::Failed(dep, w)
        }
        Outcome::Refused(w) => defer_app(ops, app, defer::BUSY, w),
    }
}

fn mark_failed(ops: &dyn WaveOps, app: &AppRow, dep: &str, why: &str) {
    set_app(&app.app_id, "failed", Some(defer::REHOST_FAILED), Some(why.to_string()), Some(dep));
    ops.audit(&app.app_id, "rehost_failed", json!({ "deployment": dep, "why": why, "k2Version": ops.k2_version() }));
    if once(&app.app_id, &format!("failed:{}", ops.k2_version())) {
        ops.agent_note(
            app,
            &format!(
                "[k2 apps] App {}: K2 updated to {} and its re-host onto the new runtime failed ({why}); it keeps running on its \
                 previous instance. K2 won't try again on this version; k2 app rehost {} does. Read why: k2 app history {}",
                app.app_id,
                ops.k2_version(),
                app.app_id,
                app.app_id
            ),
        );
    }
}

fn wave_failures() -> &'static Mutex<u32> {
    static F: Mutex<u32> = Mutex::new(0);
    &F
}

/// `/boot-status` `appEngine.apps` (R7): Apps whose live instance runs on
/// an older K2 or engine, and the wave's not-yet-done ones. `None` before
/// the DB is open.
pub fn counts() -> Option<(usize, usize)> {
    let h = k2_core::db::try_shared()?;
    let apps = {
        let c = h.lock();
        store::list_apps(&c, None).ok()?
    };
    let ops = LiveOps;
    let old = apps.iter().filter(|a| due(&ops, a).is_some()).count();
    let pending = with_status(|s| s.apps.values().filter(|a| matches!(a.state.as_str(), "due" | "running" | "deferred")).count());
    Some((old, pending))
}

/// One wave over every App. Returns the turns taken (tests).
pub fn run_wave(ops: &dyn WaveOps) -> Vec<(String, Turn)> {
    // Pre-flight 0 (R36): the predecessor has let go of the App ports.
    while ops.predecessor_draining() {
        set_wave("waiting", Some(defer::PREDECESSOR_DRAINING.into()));
        ops.sleep(Duration::from_secs(5));
    }
    while !ops.engine_ready() {
        set_wave("waiting", Some("the App engine heal".into()));
        ops.sleep(Duration::from_secs(5));
    }
    // Pre-flight 1 (R29, Q11): core upgrades first, at most 30 minutes.
    let start = Instant::now();
    let mut waited = Duration::ZERO;
    while let Some(what) = ops.core_busy() {
        if waited >= CORE_WAIT {
            k2_core::log_debug!("[apps] rehost: {} still running after {} min; the wave starts ({})", what, CORE_WAIT.as_secs() / 60, defer::CORE_UPGRADE_WAIT);
            ops.audit("-", "rehost_core_wait", json!({ "what": what, "waitedSecs": waited.as_secs() }));
            break;
        }
        set_wave("waiting", Some(format!("{}: {what}", defer::CORE_UPGRADE_WAIT)));
        ops.sleep(Duration::from_secs(10));
        waited += Duration::from_secs(10);
    }
    let _ = start;
    let mut apps: Vec<AppRow> = ops.apps().into_iter().filter(|a| due(ops, a).is_some()).collect();
    apps.sort_by_key(|a| (ops.active_connections(a), a.app_id.clone()));
    set_wave("running", None);
    *wave_failures().lock().unwrap_or_else(|e| e.into_inner()) = 0;
    let mut out = Vec::new();
    for (i, app) in apps.iter().enumerate() {
        if i > 0 {
            ops.sleep(GAP);
        }
        // A daemon that handed off leaves the Apps to its successor (R18).
        if k2_core::handoff_state::draining() {
            set_wave("stopped", Some("this daemon handed off; the new one runs the wave".into()));
            return out;
        }
        let t = turn(ops, app, None);
        let streak = {
            let mut f = wave_failures().lock().unwrap_or_else(|e| e.into_inner());
            match &t {
                Turn::Done(_) => *f = 0,
                Turn::Failed(..) => *f += 1,
                _ => {}
            }
            *f
        };
        out.push((app.app_id.clone(), t));
        if streak >= BREAKER {
            breaker(ops);
            return out;
        }
    }
    set_wave("done", None);
    out
}

fn breaker(ops: &dyn WaveOps) {
    let why = format!("{BREAKER} re-hosts in a row failed or rolled back");
    set_wave("stopped", Some(why.clone()));
    ops.audit("-", "rehost_breaker", json!({ "why": why, "k2Version": ops.k2_version() }));
    ops.owner_notice(
        "K2 stopped moving Apps to its new runtime",
        &format!(
            "After the update to K2 {}, {why}, so K2 stopped the re-host wave. Every App keeps running on its current instance. \
             See k2 app rehost status; each App can be moved by hand with k2 app rehost <app>.",
            ops.k2_version()
        ),
    );
}

/// A13: an auto-rollback of a re-host inside its watch window (the
/// supervisor's hook): the App is held at `rehost_failed`, and the running
/// wave's breaker counts it.
pub fn on_auto_rollback(app_id: &str, deployment: &str, why: &str) {
    let ops = LiveOps;
    let streak = {
        let mut f = wave_failures().lock().unwrap_or_else(|e| e.into_inner());
        *f += 1;
        *f
    };
    if let Some(app) = ops.apps().into_iter().find(|a| a.app_id == app_id) {
        mark_failed(&ops, &app, deployment, &format!("{why} inside its watch window; rolled back"));
    }
    if streak >= BREAKER && with_status(|s| s.state == "running") {
        breaker(&ops);
    }
}

// ── live ─────────────────────────────────────────────────────────────

struct LiveOps;

fn db<R>(f: impl FnOnce(&rusqlite::Connection) -> rusqlite::Result<R>) -> Option<R> {
    let h = k2_core::db::shared();
    let c = h.lock();
    f(&c).ok()
}

fn live_manifest(app: &AppRow) -> Option<(k2_core::deploy::manifest::Manifest, store::SeedRow)> {
    let seed = db(|c| store::get_seed(c, app.live_seed.as_deref().unwrap_or("")))??;
    let m = serde_json::from_value(seed.manifest.clone()).ok()?;
    Some((m, seed))
}

impl WaveOps for LiveOps {
    fn apps(&self) -> Vec<AppRow> {
        db(|c| store::list_apps(c, None)).unwrap_or_default()
    }
    fn k2_version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }
    fn engine_version(&self) -> Option<String> {
        super::engine_gate::snapshot().running.map(|r| r.version)
    }
    fn engine_ready(&self) -> bool {
        if !super::guarded::engine_installed() {
            return true;
        }
        let g = super::engine_gate::snapshot();
        let healing = super::engine_heal::app_engine_json().is_some_and(|v| v["state"] == "healing");
        g.adopt_answered && g.matching == Some(true) && !healing
    }
    fn predecessor_draining(&self) -> bool {
        crate::handoff::booted_from_handoff() && crate::handoff::predecessor_alive()
    }
    fn core_busy(&self) -> Option<String> {
        if let Some(h) = crate::root_helper::busy::holder() {
            return Some(h.to_string());
        }
        if crate::mail::supervisor::enable_running().load(std::sync::atomic::Ordering::SeqCst) {
            return Some("a hosted-mail enable or upgrade".into());
        }
        if crate::sew::supervisor::lifecycle_busy() {
            return Some("a Sew upgrade".into());
        }
        if let Some(b) = crate::sql::pgmove::worker::busy() {
            return Some(format!("a Postgres move ({b})"));
        }
        if crate::sql::supervisor::enable_running().load(std::sync::atomic::Ordering::SeqCst) {
            return Some("a Postgres enable or upgrade".into());
        }
        None
    }
    fn minute_of_day(&self) -> u32 {
        use chrono::Timelike;
        let t = chrono::Local::now();
        t.hour() * 60 + t.minute()
    }
    fn rehost_off(&self) -> bool {
        std::env::var(rh::REHOST_ENV).is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "off" | "false" | "no"))
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
    fn live_fingerprint(&self, app: &AppRow) -> Option<rh::Fingerprint> {
        super::supervisor::live_fingerprint(app)
    }
    fn runs_unwalled(&self, app: &AppRow) -> bool {
        super::supervisor::runs_unwalled(app)
    }
    fn busy(&self, app: &AppRow) -> bool {
        super::supervisor::is_busy(&app.app_id)
    }
    fn active_connections(&self, app: &AppRow) -> usize {
        super::supervisor::active_connections(&app.app_id)
    }
    fn requires(&self, app: &AppRow) -> Result<(), String> {
        let req = live_manifest(app).and_then(|(m, _)| m.requires);
        super::requires::check_live(&app.app_id, req.as_ref()).map_err(|(m, _)| m)
    }
    fn sessions(&self, app: &AppRow) -> SessionStore {
        match live_manifest(app) {
            Some((m, seed)) => rh::sessions(m.host.sessions, &seed.seed_id, seed.version.as_deref().or(Some(m.version.as_str()))).0,
            None => SessionStore::Unknown,
        }
    }
    fn no_overlap_shape(&self, app: &AppRow) -> Option<String> {
        no_overlap_shape(app, live_manifest(app).and_then(|(m, _)| m.host.overlap))
    }
    fn headroom(&self, app: &AppRow) -> Result<(), String> {
        let avail = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| rh::mem_available(&m));
        rh::memory_headroom(super::supervisor::live_mem_current(&app.app_id), avail).map_err(|(need, room)| {
            format!("the live instance uses {} MiB and {} MiB is free after the 512 MiB floor", need >> 20, room >> 20)
        })?;
        disk_headroom(app)
    }
    fn held_failed(&self, app: &AppRow) -> bool {
        held_failed(&app.app_id, &self.k2_version())
    }
    fn rehost(&self, app: &AppRow, pause: bool, reason: &str) -> Outcome {
        let Some(seed) = app.live_seed.clone() else { return Outcome::Refused("no live Seed".into()) };
        if pause {
            super::supervisor::set_stop_first(&app.app_id);
        }
        match super::supervisor::begin(app, &seed, store::KIND_REHOST, "system", Some(reason)) {
            Ok(dep) => {
                // The gate (≤ its timeout) plus a margin; a stall is the
                // stall check's.
                let row = super::supervisor::wait_done(&dep, 30 * 60);
                match row {
                    Some(d) if d.result == store::RESULT_LIVE => Outcome::Live(dep),
                    Some(d) => Outcome::Failed(dep, d.error.unwrap_or_else(|| d.result.clone())),
                    None => Outcome::Failed(dep, "the deployment row is gone".into()),
                }
            }
            Err(f) => Outcome::Refused(f.message),
        }
    }
    fn audit(&self, app: &str, event: &str, payload: J) {
        let h = k2_core::db::shared();
        let c = h.lock();
        let _ = daudit::append(&c, daudit::SUBJECT_APP, app, event, "system", None, &payload);
    }
    fn owner_notice(&self, title: &str, body: &str) {
        super::runtime_notes::owner_notice(title, body);
    }
    fn agent_note(&self, app: &AppRow, text: &str) {
        if !app.owner_workspace.is_empty() {
            super::alerts::notify(&app.owner_workspace, text);
        }
    }
}

/// Why an App can't run two copies by its shape (A12 + Rosson Q16): extra
/// ports, the process exception, a `handover` policy or manifest hint.
pub fn no_overlap_shape(app: &AppRow, manifest_overlap: Option<k2_core::deploy::manifest::Overlap>) -> Option<String> {
    if app.is_process_exception() {
        return Some("it runs as the owner's process exception (one unit)".into());
    }
    if app.rehost_policy == rh::POLICY_HANDOVER {
        return Some("its re-host policy is handover".into());
    }
    if manifest_overlap == Some(k2_core::deploy::manifest::Overlap::Handover) {
        return Some("its manifest asks for host.overlap handover".into());
    }
    if app.path == "open" {
        let ports = db(|c| k2_core::app_grace::config::load(c, &app.app_id)).map(|cfg| cfg.tcp_ports()).unwrap_or_default();
        if !ports.is_empty() {
            return Some(format!("it holds extra ports ({})", ports.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")));
        }
    }
    None
}

/// R12 disk: the Seed's walled need plus a new base image when its digest
/// changed, against the free space K2's App paths have.
fn disk_headroom(app: &AppRow) -> Result<(), String> {
    let mut need = super::resources::deploy_need(app);
    let fp = super::supervisor::live_fingerprint(app);
    let arch = std::env::consts::ARCH;
    let want = if app.path == "open" { super::guarded::full_image_for(arch) } else { super::guarded::base_image_for(arch) };
    if let (Some(fp), Some(img)) = (fp.as_ref(), want.as_ref()) {
        if fp.image_digest.as_deref().is_some_and(|d| d != img.digest) {
            need = need.saturating_add(if app.path == "open" { 1200 << 20 } else { 300 << 20 });
        }
    }
    let free = super::box_prep::disk_free_now().into_iter().map(|(_, b)| b).min();
    match free {
        Some(f) if f < need => Err(format!("it needs about {} MiB of disk and {} MiB is free", need >> 20, f >> 20)),
        _ => Ok(()),
    }
}

/// The App's last re-host on `version` failed (its newest `rehost_failed`
/// is after its newest `rehost_done` / `rehost_requested`).
fn held_failed(app_id: &str, version: &str) -> bool {
    let rows = db(|c| daudit::list_for(c, daudit::SUBJECT_APP, app_id, 50)).unwrap_or_default();
    let mut rows = rows;
    rows.sort_by_key(|r| std::cmp::Reverse(r.seq));
    for r in rows {
        match r.event.as_str() {
            "rehost_done" | "rehost_requested" => return false,
            "rehost_failed" => return r.payload.get("k2Version").and_then(J::as_str) == Some(version),
            _ => {}
        }
    }
    false
}

fn trigger() -> &'static (Mutex<bool>, std::sync::Condvar) {
    static T: OnceLock<(Mutex<bool>, std::sync::Condvar)> = OnceLock::new();
    T.get_or_init(|| (Mutex::new(false), std::sync::Condvar::new()))
}

/// Run the wave again now (a policy change, the owner's retry).
pub fn wake() {
    let (m, cv) = trigger();
    *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
    cv.notify_all();
}

fn sleep_or_wake(d: Duration) {
    let (m, cv) = trigger();
    let mut woke = m.lock().unwrap_or_else(|e| e.into_inner());
    if !*woke {
        woke = cv.wait_timeout(woke, d).unwrap_or_else(|e| e.into_inner()).0;
    }
    *woke = false;
}

/// Boot: the wave runs once the supervisor adopted and the engine heal is
/// done, then again for deferred Apps every hour.
pub fn spawn() {
    if cfg!(test) {
        return;
    }
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let _ = std::thread::Builder::new().name("apps-rehost".into()).spawn(|| loop {
        if k2_core::deploy::enabled() {
            let turns = run_wave(&LiveOps);
            let moved = turns.iter().filter(|(_, t)| matches!(t, Turn::Done(_))).count();
            if !turns.is_empty() {
                k2_core::log_debug!("[apps] rehost wave: {} App(s) due, {moved} moved", turns.len());
            }
        }
        sleep_or_wake(RETRY_EVERY);
    });
}

// ── routes ───────────────────────────────────────────────────────────

pub const ROUTES: &[&str] = &["/cli/app/rehost", "/cli/app/rehost/policy", "/cli/app/rehost/run"];
pub const POST_ROUTES: &[&str] = &["/cli/app/rehost/policy", "/cli/app/rehost/run"];

/// The App's re-host block (`k2 app inspect`, `GET /cli/app/rehost?app=`).
pub fn app_json(app: &AppRow) -> J {
    let ops = LiveOps;
    let fp = ops.live_fingerprint(app);
    json!({
        "policy": app.rehost_policy,
        "window": app.rehost_window,
        "whenNoOverlap": app.rehost_when_no_overlap,
        "fingerprint": fp,
        "due": due(&ops, app),
        "sessions": ops.sessions(app),
        "heldFailed": ops.held_failed(app),
        "wave": with_status(|s| s.apps.get(&app.app_id).cloned()),
    })
}

/// `GET /cli/app/rehost[?app=]` (Member read): the wave and, with `app`,
/// one App's block.
pub(crate) fn status_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    {
        need_credential(who)?;
        if let Some(a) = params.get("app").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let app = app_for(who, Some(a))?;
            return Ok(json!({ "ok": true, "app": app.app_id, "rehost": app_json(&app) }));
        }
        let mut v = status_json();
        if let Some(ws) = super::routes::agent_ws(who) {
            // A passport sees its own workspace's Apps only.
            let mine: std::collections::BTreeSet<String> = LiveOps.apps().into_iter().filter(|a| a.owner_workspace == ws).map(|a| a.app_id).collect();
            if let Some(o) = v["apps"].as_object_mut() {
                o.retain(|k, _| mine.contains(k));
            }
        }
        v["ok"] = json!(true);
        Ok(v)
    }
}

/// `POST /cli/app/rehost/run {app, yes?, pause?}` (owner or switch 1 for
/// its own App: restart-class, A20): move it to a fresh instance now.
pub(crate) fn run_route(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    super::suspend::refuse_if_suspended(&app.app_id)?;
    super::ownership::refuse_if_parked(&app)?;
    if app.live_seed.is_none() {
        return Err(no(codes::NOTHING_TO_ROLL_BACK, 409, format!("{} has no Seed deployed yet", app.app_id)));
    }
    let forced = Forced { yes: super::routes::b(&v, "yes"), pause: super::routes::b(&v, "pause") };
    let ops = LiveOps;
    ops.audit(&app.app_id, "rehost_requested", json!({ "by": super::routes::actor(who), "yes": forced.yes, "pause": forced.pause }));
    match turn(&ops, &app, Some(forced)) {
        Turn::Done(dep) => {
            let mut out = super::routes::deployment_answer(&dep, 0)?;
            out["rehost"] = json!("done");
            Ok(out)
        }
        Turn::Failed(dep, why) => {
            let mut out = super::routes::deployment_answer(&dep, 0)?;
            out["rehost"] = json!("failed");
            out["why"] = json!(why);
            Ok(out)
        }
        Turn::Deferred(code, detail) => Err(no(code, 409, format!("{}: {detail}", app.app_id))),
        Turn::NotDue => Ok(json!({ "ok": true, "app": app.app_id, "rehost": "current" })),
    }
}

/// `POST /cli/app/rehost/policy {app, policy?, window?, whenNoOverlap?}`
/// (R14, A20; Rosson Q4 + Q16): the owner, a human Admin, or an agent with
/// switch 2 for its own App.
pub(crate) fn policy_route(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    if super::access::agent_level(who).is_some_and(|l| l != super::access::Level::Full) {
        return Err(no(
            codes::APPS_MANAGE_OFF,
            403,
            "an App's update policy is \"…and manage them end to end\" (switch 2): ask your owner, or set it in the App panel",
        ));
    }
    let v = json_body(body)?;
    let app = app_for(who, s(&v, "app"))?;
    let policy = s(&v, "policy");
    if let Some(p) = policy {
        if !rh::POLICIES.contains(&p) {
            return Err(no(codes::BAD_REQUEST, 400, format!("policy must be one of {}", rh::POLICIES.join("|"))));
        }
    }
    let window = match v.get("window") {
        None => None,
        Some(J::Null) => Some(None),
        Some(w) => {
            let w = w.as_str().map(str::trim).unwrap_or("");
            if rh::parse_window(w).is_none() {
                return Err(no(codes::BAD_REQUEST, 400, "window is HH:MM-HH:MM in this box's time (e.g. 02:00-05:00)"));
            }
            Some(Some(w))
        }
    };
    let effective_window = match window {
        Some(w) => w.map(str::to_string),
        None => app.rehost_window.clone(),
    };
    if policy == Some(rh::POLICY_WINDOW) && effective_window.is_none() {
        return Err(no(codes::BAD_REQUEST, 400, "policy window needs a window (HH:MM-HH:MM)"));
    }
    let no_overlap = s(&v, "whenNoOverlap");
    if let Some(n) = no_overlap {
        if n != rh::NO_OVERLAP_PAUSE && n != rh::NO_OVERLAP_DEFER {
            return Err(no(codes::BAD_REQUEST, 400, "whenNoOverlap must be pause (pause briefly and update) or defer (skip; I update it myself)"));
        }
    }
    if policy.is_none() && window.is_none() && no_overlap.is_none() {
        return Err(no(codes::BAD_REQUEST, 400, "give policy, window or whenNoOverlap"));
    }
    {
        let h = k2_core::db::shared();
        let c = h.lock();
        store::set_rehost_settings(&c, &app.app_id, policy, window, no_overlap).map_err(|e| no("db_error", 500, e.to_string()))?;
    }
    LiveOps.audit(
        &app.app_id,
        "rehost_policy",
        json!({ "by": super::routes::actor(who), "policy": policy, "window": window, "whenNoOverlap": no_overlap }),
    );
    if policy.is_some() || window.is_some() {
        wake();
    }
    let app = app_for(who, Some(&app.app_id))?;
    let mut out = json!({ "ok": true, "app": app.app_id, "rehost": app_json(&app) });
    if app.rehost_policy == rh::POLICY_HANDOVER && live_manifest(&app).and_then(|(m, _)| m.requires).is_some_and(|r| r.voice) {
        out["warning"] = json!("this App needs voice (calls): a handover pause can drop calls longer than Sew's 20 s reconnect window");
    }
    Ok(out)
}

#[cfg(test)]
#[path = "rehost_tests.rs"]
mod tests;
